;; rust-mode.scm — built-in Rust major mode, auto-enabled by find-file-hook
;; whenever the visited file has a ".rs" extension. Entirely Scheme over the
;; same public contract as dired.scm; Rust core has no idea "rust-mode"
;; exists (see taco-boundary memory). No LSP — this is editor-level
;; convenience only:
;;
;;   * tree-sitter syntax highlighting, auto-installed (git clone + compile,
;;     cached after) the first time a .rs file is opened this process —
;;     mirrors examples/treesitter-rust.scm, including its face choices.
;;   * TAB: a brace-depth indent heuristic (not a real parser — it looks at
;;     the previous non-blank line and the current line's own leading
;;     brace/paren/bracket to decide the indent level). The global
;;     indent-width (bootstrap.scm, default 4) controls the step; override
;;     it in init.scm with (set! indent-width 2). Always inserts spaces,
;;     never a tab.
;;   * RET / C-j: newline-and-indent — the new line lands already indented
;;     for its block (one level in after a `{`, dedented on a closer).
;;     Pressed between an empty pair ({|}, (|), [|]) it opens the block:
;;     the closer moves to its own line at the parent indent and point ends
;;     up on a fresh line one level in.
;;   * Electric pairing for ( ) { } [ ] and "": typing an opener inserts
;;     the closer too with point left in between; typing a closer that's
;;     already sitting right after point just moves over it instead of
;;     inserting a duplicate. Typing a closer as the first non-space
;;     character of a line snaps the line back to the block's indent.
;;   * "// " as this buffer's comment-dwim (M-;) syntax — see bootstrap.scm.

;; ---- indentation ------------------------------------------------------------

(define (rust-ends-with-opener? line)
  (let* ((t (string-rstrip line)) (n (string-length t)))
    (and (> n 0)
         (let ((c (substring t (- n 1) n)))
           (or (equal? c "{") (equal? c "(") (equal? c "["))))))

(define (rust-starts-with-closer? line)
  (let ((t (string-lstrip line)))
    (and (> (string-length t) 0)
         (let ((c (substring t 0 1)))
           (or (equal? c "}") (equal? c ")") (equal? c "]"))))))

(define (rust-prev-nonblank-index lines idx)
  (cond ((< idx 0) #f)
        ((> (string-length (string-lstrip (list-ref lines idx))) 0) idx)
        (else (rust-prev-nonblank-index lines (- idx 1)))))

(define (rust-target-indent lines idx)
  (let ((prev (rust-prev-nonblank-index lines (- idx 1))))
    (if (not prev)
        0
        (let* ((pline (list-ref lines prev))
               (base (leading-space-count pline))
               (opened (rust-ends-with-opener? pline))
               (closes (rust-starts-with-closer? (list-ref lines idx))))
          (cond ((and opened closes) base)
                (opened (+ base indent-width))
                (closes (if (< (- base indent-width) 0) 0 (- base indent-width)))
                (else base))))))

;; Only the leading-whitespace span is touched (delete-region! + insert-text,
;; both undo-recording) rather than rewriting the whole buffer through
;; set-buffer-string! — that primitive is for regenerating non-user-authored
;; content (a dired listing) and deliberately records no undo entry.
(define (rust-indent-line)
  (let* ((lines (buffer-lines))
         (idx (- (line-number) 1))
         (cur (list-ref lines idx))
         (cur-indent (leading-space-count cur))
         (target (rust-target-indent lines idx))
         (start (line-start-offset lines idx))
         (offset-in-line (- (point) start))
         (kept (if (< offset-in-line cur-indent) 0 (- offset-in-line cur-indent))))
    (delete-region! start (+ start cur-indent))
    (goto-char start)
    (insert-text (make-spaces target))
    (goto-char (+ start target kept))))

;; ---- newline ------------------------------------------------------------------

(define (rust-pair-closer open)
  (cond ((equal? open "{") "}")
        ((equal? open "(") ")")
        ((equal? open "[") "]")
        (else #f)))

;; RET / C-j. Between an empty pair — {|}, usually just typed via the
;; electric opener — this opens the block Emacs-style: the closer goes to
;; its own line at the parent indent, and point lands on a fresh line one
;; level in, ready for the body. Anywhere else it is newline + indent.
(define (rust-newline-and-indent)
  (let ((closer (rust-pair-closer (char-before))))
    (if (and closer (equal? (char-after) closer))
        (begin
          (insert-text "\n\n")
          (rust-indent-line)            ; the closer's line, at parent indent
          (let ((lines (buffer-lines)))
            (goto-char (line-start-offset lines (- (line-number) 2))))
          (rust-indent-line))           ; the body line; point ends after its indent
        (begin
          (insert-text "\n")
          (rust-indent-line)))))

;; ---- electric pairs ----------------------------------------------------------

(define (rust-electric-pair-insert open close)
  (insert-text (string-append open close))
  (goto-char (- (point) (string-length close))))

(define (rust-electric-open-paren) (rust-electric-pair-insert "(" ")"))
(define (rust-electric-open-brace) (rust-electric-pair-insert "{" "}"))
(define (rust-electric-open-bracket) (rust-electric-pair-insert "[" "]"))

;; After inserting/skipping the closer: if it is the first non-space
;; character of its line (the "}" ending a block, typed on a fresh line),
;; snap the line back to the block's indent instead of leaving it wherever
;; RET's body indent put it.
(define (rust-electric-close closer)
  (if (equal? (char-after) closer)
      (goto-char (+ (point) 1))
      (insert-text closer))
  (let* ((lines (buffer-lines))
         (idx (- (line-number) 1))
         (line (list-ref lines idx))
         (start (line-start-offset lines idx))
         (before-point (substring line 0 (- (point) start))))
    (when (equal? (string-lstrip before-point) closer)
      (rust-indent-line))))

(define (rust-electric-close-paren) (rust-electric-close ")"))
(define (rust-electric-close-brace) (rust-electric-close "}"))
(define (rust-electric-close-bracket) (rust-electric-close "]"))

(define (rust-electric-quote)
  (if (equal? (char-after) "\"")
      (goto-char (+ (point) 1))
      (rust-electric-pair-insert "\"" "\"")))

;; ---- mode setup ---------------------------------------------------------------

(define *rust-treesit-ready* #f)

;; Install/compile the grammar at most once per process (the on-disk cache
;; that tree-sit-install-language-grammar itself keeps makes this cheap on
;; the next taco run, but there is no reason to repeat the clone/compile
;; check on every .rs file opened in one session).
(define (rust-ensure-treesit!)
  (unless *rust-treesit-ready*
    (set! *rust-treesit-ready* #t)
    (tree-sit-install-language-grammar "rust" "https://github.com/tree-sitter/tree-sitter-rust")
    (tree-sit-enable-for-extension "rs" "rust")
    (set-face-color "keyword" "magenta")
    (set-face-color "string" "green")
    (set-face-color "comment" "cyan")
    (set-face-color "type" "yellow")
    (set-face-color "type.builtin" "yellow")
    (set-face-color "function" "blue")
    (set-face-color "function.method" "blue")
    (set-face-color "constant" "red")
    (set-face-color "constant.builtin" "red")))

(define (rust-mode)
  (rust-ensure-treesit!)
  (set-buffer-mode-name "Rust")
  (use-local-map "rust-mode-map")
  (buffer-local-set! "comment-start" "// ")
  ;; What C-M-h (mark-defun, bootstrap.scm) selects in a Rust buffer.
  (buffer-local-set! "defun-node-kinds" '("function_item")))

(add-hook "find-file-hook"
  (lambda ()
    (when (equal? (file-name-extension (buffer-file-name)) "rs")
      (rust-mode))))

;; ---- commands & keymap ---------------------------------------------------------

(define-command "rust-indent-line" "Indent the current line (Rust brace-depth heuristic)." rust-indent-line)
(define-command "rust-newline-and-indent" "Insert a newline and indent for the block; between an empty pair, open the block." rust-newline-and-indent)
(define-command "rust-electric-open-paren" "Insert (), cursor between." rust-electric-open-paren)
(define-command "rust-electric-close-paren" "Insert ), or skip over one already there." rust-electric-close-paren)
(define-command "rust-electric-open-brace" "Insert {}, cursor between." rust-electric-open-brace)
(define-command "rust-electric-close-brace" "Insert }, or skip over one already there." rust-electric-close-brace)
(define-command "rust-electric-open-bracket" "Insert [], cursor between." rust-electric-open-bracket)
(define-command "rust-electric-close-bracket" "Insert ], or skip over one already there." rust-electric-close-bracket)
(define-command "rust-electric-quote" "Insert \"\", cursor between, or skip over a closing quote." rust-electric-quote)

(define-key "rust-mode-map" "TAB" "rust-indent-line")
(define-key "rust-mode-map" "RET" "rust-newline-and-indent")
(define-key "rust-mode-map" "C-j" "rust-newline-and-indent")
(define-key "rust-mode-map" "(" "rust-electric-open-paren")
(define-key "rust-mode-map" ")" "rust-electric-close-paren")
(define-key "rust-mode-map" "{" "rust-electric-open-brace")
(define-key "rust-mode-map" "}" "rust-electric-close-brace")
(define-key "rust-mode-map" "[" "rust-electric-open-bracket")
(define-key "rust-mode-map" "]" "rust-electric-close-bracket")
(define-key "rust-mode-map" "\"" "rust-electric-quote")
