;; python-mode.scm — built-in Python major mode, auto-enabled by
;; find-file-hook for ".py" files. Same shape as rust-mode.scm: tree-sitter
;; highlighting (grammar auto-installed on first use), an indent heuristic,
;; electric pairs, comment syntax, and mark-defun node kinds. Rust core has
;; no idea "python-mode" exists.
;;
;;   * TAB: previous non-blank line's indent, one level deeper after a
;;     line ending in ":", one level shallower on a dedenting keyword line
;;     (else/elif/except/finally). A heuristic, not a parser.
;;   * RET / C-j: newline-and-indent.
;;   * Electric pairing for ( ) [ ] { } and "".
;;   * "# " comment syntax for M-; (comment-dwim).
;;   * C-M-h selects the enclosing def/class (mark-defun, bootstrap.scm).

;; ---- indentation ------------------------------------------------------------

(define (python-dedent-keyword? line)
  (let ((t (string-lstrip line)))
    (or (string-starts-with? t "else")
        (string-starts-with? t "elif")
        (string-starts-with? t "except")
        (string-starts-with? t "finally"))))

(define (python-target-indent lines idx)
  (let ((prev (prev-nonblank-index lines (- idx 1))))
    (if (not prev)
        0
        (let* ((pline (list-ref lines prev))
               (base (leading-space-count pline))
               (opens (string-ends-with? (string-rstrip pline) ":"))
               (dedents (python-dedent-keyword? (list-ref lines idx))))
          ;; "except:" right under "try:" belongs at the same level; only
          ;; one of the two adjustments ever applies.
          (cond ((and opens dedents) base)
                (opens (+ base indent-width))
                (dedents (max 0 (- base indent-width)))
                (else base))))))

(define (python-indent-line)
  (set-line-indent (python-target-indent (buffer-lines) (- (line-number) 1))))

(define (python-newline-and-indent)
  (insert-text "\n")
  (python-indent-line))

;; ---- electric pairs ----------------------------------------------------------

(define (python-electric-open-paren) (electric-pair-insert "(" ")"))
(define (python-electric-open-bracket) (electric-pair-insert "[" "]"))
(define (python-electric-open-brace) (electric-pair-insert "{" "}"))
(define (python-electric-close-paren) (electric-close-insert ")" #f))
(define (python-electric-close-bracket) (electric-close-insert "]" #f))
(define (python-electric-close-brace) (electric-close-insert "}" #f))
(define (python-electric-quote) (electric-quote-insert "\""))

;; ---- mode setup ---------------------------------------------------------------

(define *python-treesit-ready* #f)

(define (python-ensure-treesit!)
  (unless *python-treesit-ready*
    (set! *python-treesit-ready* #t)
    (tree-sit-install-language-grammar "python" "https://github.com/tree-sitter/tree-sitter-python")
    (tree-sit-enable-for-extension "py" "python")
    (set-face-color "keyword" "magenta")
    (set-face-color "string" "green")
    (set-face-color "comment" "cyan")
    (set-face-color "type" "yellow")
    (set-face-color "type.builtin" "yellow")
    (set-face-color "function" "blue")
    (set-face-color "function.method" "blue")
    (set-face-color "constant" "red")
    (set-face-color "constant.builtin" "red")
    (set-face-color "number" "red")))

(define (python-mode)
  (python-ensure-treesit!)
  (set-buffer-mode-name "Python")
  (use-local-map "python-mode-map")
  (buffer-local-set! "comment-start" "# ")
  (buffer-local-set! "defun-node-kinds" '("function_definition" "class_definition")))

(add-hook "find-file-hook"
  (lambda ()
    (when (equal? (file-name-extension (buffer-file-name)) "py")
      (python-mode))))

;; ---- commands & keymap ---------------------------------------------------------

(define-command "python-indent-line" "Indent the current line (Python heuristic)." python-indent-line)
(define-command "python-newline-and-indent" "Insert a newline and indent." python-newline-and-indent)
(define-command "python-electric-open-paren" "Insert (), cursor between." python-electric-open-paren)
(define-command "python-electric-close-paren" "Insert ), or skip over one already there." python-electric-close-paren)
(define-command "python-electric-open-bracket" "Insert [], cursor between." python-electric-open-bracket)
(define-command "python-electric-close-bracket" "Insert ], or skip over one already there." python-electric-close-bracket)
(define-command "python-electric-open-brace" "Insert {}, cursor between." python-electric-open-brace)
(define-command "python-electric-close-brace" "Insert }, or skip over one already there." python-electric-close-brace)
(define-command "python-electric-quote" "Insert \"\", cursor between, or skip over a closing quote." python-electric-quote)

(define-key "python-mode-map" "TAB" "python-indent-line")
(define-key "python-mode-map" "RET" "python-newline-and-indent")
(define-key "python-mode-map" "C-j" "python-newline-and-indent")
(define-key "python-mode-map" "(" "python-electric-open-paren")
(define-key "python-mode-map" ")" "python-electric-close-paren")
(define-key "python-mode-map" "[" "python-electric-open-bracket")
(define-key "python-mode-map" "]" "python-electric-close-bracket")
(define-key "python-mode-map" "{" "python-electric-open-brace")
(define-key "python-mode-map" "}" "python-electric-close-brace")
(define-key "python-mode-map" "\"" "python-electric-quote")
