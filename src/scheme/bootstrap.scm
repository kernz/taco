;; taco bootstrap — the default keymap, built entirely through the public
;; Scheme contract. User config (~/.config/taco/init.scm) runs after this
;; and can rebind anything with the same functions.

;; ---- Appearance ---------------------------------------------------------
(set-face-color "mode-line" "white")
(set-face-color "highlight" "white")

;; ---- Basics & system --------------------------------------------------
(global-set-key "C-x C-c" "save-buffers-kill-terminal")
(global-set-key "C-x C-s" "save-buffer")
(global-set-key "C-x b"   "switch-to-buffer")
(global-set-key "C-x k"   "kill-buffer")
(global-set-key "C-x C-f" "find-file")
(global-set-key "C-/"     "undo")

;; ---- Movement -----------------------------------------------------------
(global-set-key "M-<" "beginning-of-buffer")
(global-set-key "M->" "end-of-buffer")
(global-set-key "C-v" "scroll-up-command")
(global-set-key "M-v" "scroll-down-command")
(global-set-key "C-l" "recenter")
(global-set-key "C-a" "beginning-of-line")
(global-set-key "C-e" "end-of-line")
(global-set-key "C-n" "next-line")
(global-set-key "C-p" "previous-line")
(global-set-key "M-g g" "goto-line")
(global-set-key "M-f" "forward-word")
(global-set-key "M-b" "backward-word")
(global-set-key "C-f" "forward-char")
(global-set-key "C-b" "backward-char")

;; ---- Searching & editing -----------------------------------------------
(global-set-key "C-s" "isearch-forward")
(global-set-key "C-r" "isearch-backward")
(global-set-key "M-%" "query-replace")
(global-set-key "TAB" "indent-line")
(global-set-key "C-j" "newline-and-indent")
(global-set-key "M-\\" "delete-horizontal-space")
(global-set-key "C-o" "open-line")
(global-set-key "C-d" "delete-char")
(global-set-key "M-backspace" "backward-kill-word")
;; Needs the kitty keyboard protocol: legacy terminals send C-backspace as a
;; bare DEL, which falls through to delete-backward-char.
(global-set-key "C-<backspace>" "backward-kill-word")
(global-set-key "C-x SPC" "rectangle-mark-mode")
(global-set-key "C-x r t" "string-rectangle")

;; ---- Kill ring ------------------------------------------------------------
(global-set-key "C-SPC" "set-mark-command")
(global-set-key "M-w" "kill-ring-save")
(global-set-key "C-w" "kill-region")
(global-set-key "C-y" "yank")
(global-set-key "M-y" "yank-pop")
(global-set-key "M-d" "kill-word")
(global-set-key "C-k" "kill-line")

;; ---- Comments ---------------------------------------------------------
(global-set-key "M-;" "comment-dwim")

(global-set-key "M-x" "execute-extended-command")

;; ---- Formatting & windows -------------------------------------------------
(global-set-key "C-t" "transpose-chars")
(global-set-key "M-u" "upcase-word")
(global-set-key "M-l" "downcase-word")
(global-set-key "C-x o" "other-window")
(global-set-key "C-x 1" "delete-other-windows")
(global-set-key "C-x 2" "split-window-below")
(global-set-key "C-x 3" "split-window-right")
(global-set-key "C-x 0" "delete-window")
;; The C-h help system (describe-key/-command/-variable, apropos, C-h ?)
;; lives in help.scm, loaded after compile.scm.

;; Dired's entry-point bindings (C-x C-j, C-c f d, ...), its mode map, and
;; the wgrep map live in dired.scm (loaded right after this file) along with
;; every other dired command — see the boundary note there.

;; ---- Hooks -----------------------------------------------------------------------
;; Emacs-style named hooks. The native core fires, per input event:
;;   "minibuffer-setup-hook"  — a prompt just opened
;;   "post-command-hook"      — a key was handled while a prompt is active
;;   "minibuffer-exit-hook"   — the prompt closed (RET, C-g, exit-minibuffer)
;;   "find-file-hook"         — find-file just visited a real file (not a
;;                              directory — that goes through dired.scm's
;;                              register-directory-opener instead)
;; Completion UIs (see examples/vertico.scm) hang off the first three,
;; together with (minibuffer-contents), (minibuffer-completion-kind),
;; (minibuffer-set-key) and (minibuffer-show-candidates). Unlike Emacs,
;; (remove-hook name) takes no function argument: it removes every function
;; on the named hook.

(define *hooks* '())

;; (fn is Steel's lambda alias, hence the `f` parameter name)
(define (add-hook name f)
  (set! *hooks* (cons (cons name f) *hooks*)))

(define (remove-hook name)
  (set! *hooks* (filter (lambda (h) (not (equal? (car h) name))) *hooks*)))

(define (run-hooks name)
  (for-each (lambda (h) (when (equal? (car h) name) ((cdr h))))
            *hooks*))

;; ---- Tree-sitter: extension -> language policy -----------------------------------
;; Rust only knows how to install/compile/highlight a grammar by name (see
;; tree-sit-install-language-grammar / tree-sit-enable); which file
;; extension should use which installed language is this alist, exactly the
;; kind of policy that belongs in Scheme rather than a Rust match arm.
;; Usage from init.scm:
;;   (tree-sit-install-language-grammar "rust" "https://github.com/tree-sitter/tree-sitter-rust")
;;   (tree-sit-enable-for-extension "rs" "rust")

(define *tree-sit-extensions* '())

(define (tree-sit-enable-for-extension ext lang)
  (set! *tree-sit-extensions* (cons (cons ext lang) *tree-sit-extensions*)))

(add-hook "find-file-hook"
  (lambda ()
    (let ((ext (file-name-extension (buffer-file-name))))
      (unless (equal? ext "")
        (let ((entry (assoc ext *tree-sit-extensions*)))
          (when entry (tree-sit-enable (cdr entry))))))))

;; ---- mark-defun (C-M-h) ----------------------------------------------------
;; Set the region around the function/definition at point. Purely
;; data-driven: a mode opts in with
;;   (buffer-local-set! "defun-node-kinds" '("function_definition" ...))
;; naming the tree-sitter node kinds that count as a defun, plus optionally
;;   (buffer-local-set! "defun-outermost" #t)
;; for lisps, where every nested form shares one node kind and the defun is
;; the outermost one. Buffers without a tree-sitter mode have nothing to
;; query, so the command just explains itself there.
(define (mark-defun)
  (let ((kinds (buffer-local-get "defun-node-kinds")))
    (if (equal? kinds #f)
        (message "mark-defun: no defun node kinds here (needs a tree-sitter mode)")
        (let ((range (tree-sit-node-range-at-point
                      kinds
                      (equal? (buffer-local-get "defun-outermost") #t))))
          (if (< (car range) 0)
              (message "No defun at point")
              (begin
                (goto-char (car range))
                (set-mark (list-ref range 1))))))))

(define-command "mark-defun"
  "Set region around the defun at point (tree-sitter modes only)."
  mark-defun)
(global-set-key "C-M-h" "mark-defun")

;; ---- Indentation -----------------------------------------------------------
;; Global indentation step, in spaces — one "level" of indent is this many
;; columns, and indenting always inserts spaces, never a tab character.
;; Every Scheme indent command steps by it (rust-mode's TAB/RET, and any
;; future mode should too). Override in init.scm: (set! indent-width 2).

(define indent-width 4)

;; ---- Small generic string/line helpers -------------------------------------
;; Steel's string library is thin (see taco-boundary memory), so any mode
;; that wants to reason about lines/indentation (rust-mode.scm, comment-dwim
;; below) shares these instead of re-deriving them per file.

(define (make-spaces n)
  (if (<= n 0) "" (string-append " " (make-spaces (- n 1)))))

(define (leading-space-count s)
  (let loop ((i 0))
    (if (and (< i (string-length s)) (equal? (substring s i (+ i 1)) " "))
        (loop (+ i 1))
        i)))

(define (string-lstrip s) (substring s (leading-space-count s) (string-length s)))

(define (trailing-space-count s)
  (let loop ((i (string-length s)))
    (if (and (> i 0) (equal? (substring s (- i 1) i) " "))
        (loop (- i 1))
        (- (string-length s) i))))

(define (string-rstrip s)
  (substring s 0 (- (string-length s) (trailing-space-count s))))

(define (string-starts-with? s prefix)
  (and (>= (string-length s) (string-length prefix))
       (equal? (substring s 0 (string-length prefix)) prefix)))

(define (string-ends-with? s suffix)
  (and (>= (string-length s) (string-length suffix))
       (equal? (substring s (- (string-length s) (string-length suffix))
                 (string-length s))
               suffix)))

;; Split on a single-char separator, keeping empty fields:
;; ("a\nb\n" "\n") -> ("a" "b" ""). The trailing "" is how a caller
;; consuming streamed lines can tell a terminated line from a partial one.
(define (string-split-char s sep)
  (let loop ((start 0) (acc '()))
    (let ((at (string-index-of s sep start)))
      (if at
          (loop (+ at 1) (cons (substring s start at) acc))
          (reverse (cons (substring s start (string-length s)) acc))))))

;; First index >= start where `sub` occurs in `s`, or #f.
(define (string-index-of s sub start)
  (let ((sl (string-length s)) (nl (string-length sub)))
    (let loop ((i start))
      (cond ((> (+ i nl) sl) #f)
            ((equal? (substring s i (+ i nl)) sub) i)
            (else (loop (+ i 1)))))))

;; Char offset of the start of `(list-ref lines idx)`, i.e. as if `lines`
;; (from buffer-lines) were joined back with "\n" — the buffer's own line
;; boundaries, for positioning goto-char/delete-region! against a specific
;; line without touching the rest of the buffer.
(define (line-start-offset lines idx)
  (if (<= idx 0) 0
      (+ (line-start-offset lines (- idx 1))
         (string-length (list-ref lines (- idx 1))) 1)))

;; 0-based index of the line containing char offset `pos`.
(define (char-to-line-index lines pos)
  (let loop ((ls lines) (offset 0) (idx 0))
    (cond ((null? ls) idx)
          ((< pos (+ offset (string-length (car ls)) 1)) idx)
          (else (loop (cdr ls) (+ offset (string-length (car ls)) 1) (+ idx 1))))))

;; ---- Generic language-mode helpers ------------------------------------------
;; Shared by the built-in language modes (python-mode.scm, c-mode.scm,
;; scheme-mode.scm). rust-mode.scm predates these and keeps its own copies.

;; Previous non-blank line index at or before `idx`, or #f.
(define (prev-nonblank-index lines idx)
  (cond ((< idx 0) #f)
        ((> (string-length (string-lstrip (list-ref lines idx))) 0) idx)
        (else (prev-nonblank-index lines (- idx 1)))))

;; Replace the current line's leading whitespace with `target` spaces,
;; keeping point at the same position within the line's text. Only the
;; leading-whitespace span is touched (delete-region! + insert-text, both
;; undo-recording), never a whole-buffer rewrite.
(define (set-line-indent target)
  (let* ((lines (buffer-lines))
         (idx (- (line-number) 1))
         (cur (list-ref lines idx))
         (cur-indent (leading-space-count cur))
         (start (line-start-offset lines idx))
         (offset-in-line (- (point) start))
         (kept (if (< offset-in-line cur-indent) 0 (- offset-in-line cur-indent))))
    (delete-region! start (+ start cur-indent))
    (goto-char start)
    (insert-text (make-spaces target))
    (goto-char (+ start target kept))))

;; Electric pairs: insert open+close with point in between; typing a closer
;; that's already at point skips over it. `reindent` (a thunk or #f) runs
;; when the closer is the first non-space char of its line, so block-based
;; modes can snap it back to the block's indent.
(define (electric-pair-insert open close)
  (insert-text (string-append open close))
  (goto-char (- (point) (string-length close))))

(define (electric-close-insert closer reindent)
  (if (equal? (char-after) closer)
      (goto-char (+ (point) 1))
      (insert-text closer))
  (when reindent
    (let* ((lines (buffer-lines))
           (idx (- (line-number) 1))
           (line (list-ref lines idx))
           (start (line-start-offset lines idx))
           (before-point (substring line 0 (- (point) start))))
      (when (equal? (string-lstrip before-point) closer)
        (reindent)))))

(define (electric-quote-insert q)
  (if (equal? (char-after) q)
      (goto-char (+ (point) 1))
      (electric-pair-insert q q)))

;; ---- Comment toggling (M-;) -------------------------------------------------
;; Emacs' comment-dwim, mode-agnostic: a mode sets the buffer-local
;; "comment-start" (e.g. rust-mode.scm sets "// "); this only knows how to
;; use it, not which language it's in. With no region: toggles to/inserts a
;; trailing comment on the current line, or hops into one that's already
;; there — exactly comment-indent's behavior, not "comment out the whole
;; line" (that's what the region case is for). With a region (mark-active?):
;; comments/uncomments every line the region spans, like comment-region /
;; uncomment-region.

(define (comment-dwim)
  (let ((cs (buffer-local-get "comment-start")))
    (if (equal? cs #f)
        (read-string "Commenting syntax is not defined. Use: " "" ""
                     (lambda (s)
                       (buffer-local-set! "comment-start" s)
                       (comment-dwim-run s)))
        (comment-dwim-run cs))))

(define (comment-dwim-run cs)
  (if (mark-active?)
      (comment-dwim-region cs)
      (comment-dwim-line cs)))

(define (comment-dwim-region cs)
  (let* ((lines (buffer-lines))
         (a (if (< (point) (mark)) (point) (mark)))
         (b (if (< (point) (mark)) (mark) (point)))
         (i0 (char-to-line-index lines a))
         (i1 (char-to-line-index lines b))
         (trimmed (string-rstrip cs))
         (uncomment? (comment-dwim-all-commented? lines i0 i1 trimmed)))
    (comment-dwim-apply-range i0 i1
      (if uncomment?
          (lambda (l) (comment-dwim-strip-line l trimmed))
          (lambda (l) (comment-dwim-add-line l cs))))))

(define (comment-dwim-all-commented? lines i0 i1 trimmed)
  (or (> i0 i1)
      (and (string-starts-with? (string-lstrip (list-ref lines i0)) trimmed)
           (comment-dwim-all-commented? lines (+ i0 1) i1 trimmed))))

;; Edits lines [i0, i1] top-to-bottom via delete-region!/insert-text (both
;; undo-recording — see the delete-region! doc comment in mod.rs) instead of
;; set-buffer-string!, which would silently make the whole toggle
;; un-undoable. Re-reads (buffer-lines) after every line since each edit
;; shifts char offsets for everything after it; a comment-toggle region is
;; small, so this isn't a hot loop.
(define (comment-dwim-apply-range i0 i1 f)
  (when (<= i0 i1)
    (let* ((lines (buffer-lines))
           (line (list-ref lines i0))
           (start (line-start-offset lines i0))
           (new-line (f line)))
      (delete-region! start (+ start (string-length line)))
      (goto-char start)
      (insert-text new-line))
    (comment-dwim-apply-range (+ i0 1) i1 f)))

(define (comment-dwim-add-line line cs)
  (let ((ind (leading-space-count line)))
    (string-append (substring line 0 ind) cs (substring line ind (string-length line)))))

(define (comment-dwim-strip-line line trimmed)
  (let* ((ind (leading-space-count line))
         (rest (substring line ind (string-length line))))
    (if (string-starts-with? rest trimmed)
        (let ((after (substring rest (string-length trimmed) (string-length rest))))
          (string-append (substring line 0 ind)
                          (if (string-starts-with? after " ")
                              (substring after 1 (string-length after))
                              after)))
        line)))

(define (comment-dwim-line cs)
  (let* ((idx (- (line-number) 1))
         (lines (buffer-lines))
         (line (list-ref lines idx))
         (start (line-start-offset lines idx))
         (trimmed (string-rstrip cs))
         (at (string-index-of line trimmed 0)))
    (if at
        (goto-char (+ start at (string-length cs)))
        (begin
          (goto-char (+ start (string-length line)))
          (insert-text (string-append " " cs))))))

(define-command "comment-dwim"
  "Comment/uncomment the region, or toggle a trailing comment on the current line."
  comment-dwim)

;; ---- Runtime evaluation (M-:, C-x C-e, eval-buffer, load-file) --------------------
;; The editor's own interpreter, reachable from inside the editor — what
;; makes taco hackable live, Emacs-style: redefine any Scheme-level command
;; or helper in a scratch buffer, evaluate it, and the running editor
;; changes immediately. eval-string and load are Steel's own primitives,
;; executing inside the already-active VM (no Rust round-trip involved).

;; Start offset of the s-expression ending exactly at the end of `text`,
;; or #f. A tiny forward reader — forward, because a backward scan cannot
;; know whether a paren sits inside a string literal — tracking nesting,
;; strings (with \" escapes) and ; comments. Limitation: a trailing
;; comment between the sexp and point hides the sexp.
(define (sexp-start-before text)
  (let ((len (string-length text)))
    (let loop ((i 0) (stack '()) (tok #f) (in-str #f) (esc #f) (in-cmt #f) (last #f))
      (if (>= i len)
          (let ((last (cond (in-str #f)              ; unterminated string
                            (tok (cons tok len))     ; atom runs to the end
                            (else last))))
            (if (and last (= (cdr last) len))
                (sexp-prefix-start text (car last))
                #f))
          (let ((c (substring text i (+ i 1))))
            (cond
             (in-cmt (loop (+ i 1) stack #f #f #f (not (equal? c "\n")) last))
             (in-str
              (cond (esc (loop (+ i 1) stack tok #t #f #f last))
                    ((equal? c "\\") (loop (+ i 1) stack tok #t #t #f last))
                    ((equal? c "\"") (loop (+ i 1) stack #f #f #f #f (cons tok (+ i 1))))
                    (else (loop (+ i 1) stack tok #t #f #f last))))
             ((equal? c "\"")
              (loop (+ i 1) stack i #t #f #f (if tok (cons tok i) last)))
             ((equal? c ";")
              (loop (+ i 1) stack #f #f #f #t (if tok (cons tok i) last)))
             ((equal? c "(")
              (loop (+ i 1) (cons i stack) #f #f #f #f (if tok (cons tok i) last)))
             ((equal? c ")")
              (let ((last (if tok (cons tok i) last)))
                (if (null? stack)
                    (loop (+ i 1) stack #f #f #f #f last) ; stray closer
                    (loop (+ i 1) (cdr stack) #f #f #f #f
                          (cons (car stack) (+ i 1))))))
             ((member c '(" " "\n" "\t"))
              (loop (+ i 1) stack #f #f #f #f (if tok (cons tok i) last)))
             (else
              (loop (+ i 1) stack (if tok tok i) #f #f #f last))))))))

;; Reader prefixes (' ` ,) stick to the form they quote.
(define (sexp-prefix-start text start)
  (if (and (> start 0)
           (member (substring text (- start 1) start) '("'" "`" ",")))
      (sexp-prefix-start text (- start 1))
      start))

(define (last-sexp-before-point)
  (let* ((upto (substring (buffer-string) 0 (point)))
         (end (let loop ((i (string-length upto)))
                (if (and (> i 0)
                         (member (substring upto (- i 1) i) '(" " "\n" "\t")))
                    (loop (- i 1))
                    i)))
         (text (substring upto 0 end))
         (start (sexp-start-before text)))
    (if (equal? start #f) "" (substring text start end))))

(define (eval-expression)
  (read-string "Eval: " "" ""
    (lambda (src)
      (unless (equal? src "")
        (message (to-string (eval-string src)))))))

(define (eval-last-sexp)
  (let ((src (last-sexp-before-point)))
    (if (equal? src "")
        (message "No s-expression before point")
        (message (to-string (eval-string src))))))

(define (eval-buffer)
  (eval-string (buffer-string))
  (message (string-append "Evaluated buffer " (current-buffer))))

(define (load-file)
  (read-string "Load file: " "" "file"
    (lambda (path)
      (let ((p (resolve-path path)))
        (if (file-exists? p)
            (begin
              (load p)
              (message (string-append "Loaded " p)))
            (message (string-append "No such file: " p)))))))

(define-command "eval-expression"
  "Evaluate a Scheme expression and echo its value."
  eval-expression)
(define-command "eval-last-sexp"
  "Evaluate the s-expression before point and echo its value."
  eval-last-sexp)
(define-command "eval-buffer"
  "Evaluate the whole current buffer as Scheme."
  eval-buffer)
(define-command "load-file"
  "Load a Scheme file into the running editor."
  load-file)

(global-set-key "M-:" "eval-expression")
(global-set-key "C-x C-e" "eval-last-sexp")
