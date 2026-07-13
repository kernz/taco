;; scheme-mode.scm — built-in Scheme/lisp major mode, auto-enabled by
;; find-file-hook for ".scm", ".ss" and ".sld" files (taco's own init.scm
;; included). Highlighting comes from the community grammar
;; https://github.com/6cdh/tree-sitter-scheme (auto-installed on first
;; use). Same shape as the other built-in modes.
;;
;;   * TAB: indent to the innermost unclosed paren's column + 2 — a stack
;;     scan of the preceding lines that skips strings and ";" comments.
;;     An approximation of lisp-indent-function (no per-operator special
;;     forms, and multi-line strings confuse it).
;;   * RET / C-j: newline-and-indent.
;;   * Electric pairing for ( ) and "".
;;   * ";; " comment syntax for M-; (comment-dwim).
;;   * C-M-h selects the enclosing *top-level* form: in this grammar every
;;     parenthesized form is a "list" node, so mark-defun uses the
;;     outermost enclosing one (defun-outermost, bootstrap.scm).

;; ---- indentation ------------------------------------------------------------

;; Update the stack of open-paren columns over one line. In-string state
;; does not survive the line (documented approximation).
(define (scheme-scan-line line stack)
  (let ((n (string-length line)))
    (let loop ((j 0) (stack stack) (in-string #f))
      (if (>= j n)
          stack
          (let ((c (substring line j (+ j 1))))
            (cond (in-string
                   (cond ((equal? c "\\") (loop (+ j 2) stack #t))
                         ((equal? c "\"") (loop (+ j 1) stack #f))
                         (else (loop (+ j 1) stack #t))))
                  ((equal? c "\"") (loop (+ j 1) stack #t))
                  ((equal? c ";") stack)
                  ((or (equal? c "(") (equal? c "[")) (loop (+ j 1) (cons j stack) #f))
                  ((or (equal? c ")") (equal? c "]"))
                   (loop (+ j 1) (if (null? stack) stack (cdr stack)) #f))
                  (else (loop (+ j 1) stack #f))))))))

(define (scheme-target-indent lines idx)
  (let loop ((i 0) (stack '()))
    (if (>= i idx)
        (if (null? stack) 0 (+ (car stack) 2))
        (loop (+ i 1) (scheme-scan-line (list-ref lines i) stack)))))

(define (scheme-indent-line)
  (set-line-indent (scheme-target-indent (buffer-lines) (- (line-number) 1))))

(define (scheme-newline-and-indent)
  (insert-text "\n")
  (scheme-indent-line))

;; ---- electric pairs ----------------------------------------------------------

(define (scheme-electric-open-paren) (electric-pair-insert "(" ")"))
(define (scheme-electric-close-paren) (electric-close-insert ")" #f))
(define (scheme-electric-quote) (electric-quote-insert "\""))

;; ---- mode setup ---------------------------------------------------------------

(define *scheme-treesit-ready* #f)

(define (scheme-ensure-treesit!)
  (unless *scheme-treesit-ready*
    (set! *scheme-treesit-ready* #t)
    (tree-sit-install-language-grammar "scheme" "https://github.com/6cdh/tree-sitter-scheme")
    (tree-sit-enable-for-extension "scm" "scheme")
    (tree-sit-enable-for-extension "ss" "scheme")
    (tree-sit-enable-for-extension "sld" "scheme")
    (set-face-color "keyword" "magenta")
    (set-face-color "string" "green")
    (set-face-color "comment" "cyan")
    (set-face-color "function" "blue")
    (set-face-color "constant" "red")
    (set-face-color "constant.builtin" "red")
    (set-face-color "number" "red")
    (set-face-color "boolean" "red")
    (set-face-color "character" "green")
    (set-face-color "operator" "yellow")
    (set-face-color "escape" "yellow")))

(define (scheme-mode)
  (scheme-ensure-treesit!)
  (set-buffer-mode-name "Scheme")
  (use-local-map "scheme-mode-map")
  (buffer-local-set! "comment-start" ";; ")
  ;; Every form is a "list" node; the defun is the outermost one at point.
  (buffer-local-set! "defun-node-kinds" '("list"))
  (buffer-local-set! "defun-outermost" #t))

(add-hook "find-file-hook"
  (lambda ()
    (let ((ext (file-name-extension (buffer-file-name))))
      (when (or (equal? ext "scm") (equal? ext "ss") (equal? ext "sld"))
        (scheme-mode)))))

;; ---- commands & keymap ---------------------------------------------------------

(define-command "scheme-indent-line" "Indent the current line (paren-depth heuristic)." scheme-indent-line)
(define-command "scheme-newline-and-indent" "Insert a newline and indent." scheme-newline-and-indent)
(define-command "scheme-electric-open-paren" "Insert (), cursor between." scheme-electric-open-paren)
(define-command "scheme-electric-close-paren" "Insert ), or skip over one already there." scheme-electric-close-paren)
(define-command "scheme-electric-quote" "Insert \"\", cursor between, or skip over a closing quote." scheme-electric-quote)

(define-key "scheme-mode-map" "TAB" "scheme-indent-line")
(define-key "scheme-mode-map" "RET" "scheme-newline-and-indent")
(define-key "scheme-mode-map" "C-j" "scheme-newline-and-indent")
(define-key "scheme-mode-map" "(" "scheme-electric-open-paren")
(define-key "scheme-mode-map" ")" "scheme-electric-close-paren")
(define-key "scheme-mode-map" "\"" "scheme-electric-quote")
