;; c-mode.scm — built-in C major mode, auto-enabled by find-file-hook for
;; ".c" and ".h" files. Same shape as rust-mode.scm: tree-sitter
;; highlighting (grammar auto-installed on first use), a brace-depth
;; indent heuristic, electric pairs (RET between {} opens the block),
;; comment syntax, and mark-defun node kinds.

;; ---- indentation ------------------------------------------------------------

(define (c-ends-with-opener? line)
  (let* ((t (string-rstrip line)) (n (string-length t)))
    (and (> n 0)
         (let ((c (substring t (- n 1) n)))
           (or (equal? c "{") (equal? c "(") (equal? c "["))))))

(define (c-starts-with-closer? line)
  (let ((t (string-lstrip line)))
    (and (> (string-length t) 0)
         (let ((c (substring t 0 1)))
           (or (equal? c "}") (equal? c ")") (equal? c "]"))))))

(define (c-target-indent lines idx)
  (let ((prev (prev-nonblank-index lines (- idx 1))))
    (if (not prev)
        0
        (let* ((pline (list-ref lines prev))
               (base (leading-space-count pline))
               (opened (c-ends-with-opener? pline))
               (closes (c-starts-with-closer? (list-ref lines idx))))
          (cond ((and opened closes) base)
                (opened (+ base indent-width))
                (closes (max 0 (- base indent-width)))
                (else base))))))

(define (c-indent-line)
  (set-line-indent (c-target-indent (buffer-lines) (- (line-number) 1))))

(define (c-pair-closer open)
  (cond ((equal? open "{") "}")
        ((equal? open "(") ")")
        ((equal? open "[") "]")
        (else #f)))

;; RET / C-j: between an empty pair ({|}) this opens the block Emacs-style
;; — closer on its own line at the parent indent, point one level in.
(define (c-newline-and-indent)
  (let ((closer (c-pair-closer (char-before))))
    (if (and closer (equal? (char-after) closer))
        (begin
          (insert-text "\n\n")
          (c-indent-line)               ; the closer's line, at parent indent
          (let ((lines (buffer-lines)))
            (goto-char (line-start-offset lines (- (line-number) 2))))
          (c-indent-line))              ; the body line
        (begin
          (insert-text "\n")
          (c-indent-line)))))

;; ---- electric pairs ----------------------------------------------------------

(define (c-electric-open-paren) (electric-pair-insert "(" ")"))
(define (c-electric-open-brace) (electric-pair-insert "{" "}"))
(define (c-electric-open-bracket) (electric-pair-insert "[" "]"))
(define (c-electric-close-paren) (electric-close-insert ")" c-indent-line))
(define (c-electric-close-brace) (electric-close-insert "}" c-indent-line))
(define (c-electric-close-bracket) (electric-close-insert "]" c-indent-line))
(define (c-electric-quote) (electric-quote-insert "\""))

;; ---- mode setup ---------------------------------------------------------------

(define *c-treesit-ready* #f)

(define (c-ensure-treesit!)
  (unless *c-treesit-ready*
    (set! *c-treesit-ready* #t)
    (tree-sit-install-language-grammar "c" "https://github.com/tree-sitter/tree-sitter-c")
    (tree-sit-enable-for-extension "c" "c")
    (tree-sit-enable-for-extension "h" "c")
    (set-face-color "keyword" "magenta")
    (set-face-color "string" "green")
    (set-face-color "comment" "cyan")
    (set-face-color "type" "yellow")
    (set-face-color "type.builtin" "yellow")
    (set-face-color "function" "blue")
    (set-face-color "constant" "red")
    (set-face-color "constant.builtin" "red")
    (set-face-color "number" "red")
    (set-face-color "property" "cyan")
    (set-face-color "label" "magenta")
    (set-face-color "delimiter" "white")))

(define (c-mode)
  (c-ensure-treesit!)
  (set-buffer-mode-name "C")
  (use-local-map "c-mode-map")
  (buffer-local-set! "comment-start" "// ")
  (buffer-local-set! "defun-node-kinds" '("function_definition")))

(add-hook "find-file-hook"
  (lambda ()
    (let ((ext (file-name-extension (buffer-file-name))))
      (when (or (equal? ext "c") (equal? ext "h"))
        (c-mode)))))

;; ---- commands & keymap ---------------------------------------------------------

(define-command "c-indent-line" "Indent the current line (C brace-depth heuristic)." c-indent-line)
(define-command "c-newline-and-indent" "Insert a newline and indent for the block; between an empty pair, open the block." c-newline-and-indent)
(define-command "c-electric-open-paren" "Insert (), cursor between." c-electric-open-paren)
(define-command "c-electric-close-paren" "Insert ), or skip over one already there." c-electric-close-paren)
(define-command "c-electric-open-brace" "Insert {}, cursor between." c-electric-open-brace)
(define-command "c-electric-close-brace" "Insert }, or skip over one already there." c-electric-close-brace)
(define-command "c-electric-open-bracket" "Insert [], cursor between." c-electric-open-bracket)
(define-command "c-electric-close-bracket" "Insert ], or skip over one already there." c-electric-close-bracket)
(define-command "c-electric-quote" "Insert \"\", cursor between, or skip over a closing quote." c-electric-quote)

(define-key "c-mode-map" "TAB" "c-indent-line")
(define-key "c-mode-map" "RET" "c-newline-and-indent")
(define-key "c-mode-map" "C-j" "c-newline-and-indent")
(define-key "c-mode-map" "(" "c-electric-open-paren")
(define-key "c-mode-map" ")" "c-electric-close-paren")
(define-key "c-mode-map" "{" "c-electric-open-brace")
(define-key "c-mode-map" "}" "c-electric-close-brace")
(define-key "c-mode-map" "[" "c-electric-open-bracket")
(define-key "c-mode-map" "]" "c-electric-close-bracket")
(define-key "c-mode-map" "\"" "c-electric-quote")
