;; help.scm — the C-h help system, entirely in Steel, mirroring the Emacs
;; tutorial's help chapter on taco's contract. Rust supplies only narrow
;; introspection primitives: (command-doc name), (command-bindings name),
;; (buffer-local-keys), (command-names), and the interactive
;; (read-key-sequence prompt f) capture that C-h c / C-h k are built on.
;;
;;   C-h c KEYS   describe-key-briefly — echo "KEYS runs the command NAME"
;;   C-h k KEYS   describe-key — full documentation in a *Help* window
;;   C-h x NAME   describe-command (C-h f is an alias: taco only has
;;                commands, there are no non-command functions to describe)
;;   C-h v NAME   describe-variable — buffer-locals and editor options
;;   C-h a REGEX  command apropos — matching commands with their bindings
;;   C-h ?        this overview (also M-x help)
;;
;; Help appears in the other window; you stay where you were. Dismiss it
;; with C-x 1, or q inside the *Help* window. C-g cancels a pending C-h.

;; ---- the *Help* window ---------------------------------------------------------

;; Fill *Help* in the other window (Emacs' with-help-window shape) and
;; come back — the tutorial's contract is that you keep editing while the
;; help text stays visible.
(define (help-display text)
  (other-window-or-split)
  (switch-to-buffer "*Help*")
  (set-buffer-string! text)
  (goto-char 0)
  (set-buffer-read-only! #t)
  (set-buffer-mode-name "Help")
  (use-local-map "help-mode-map")
  (other-window))

;; ---- formatting helpers ----------------------------------------------------------

(define (help-pad s w)
  (if (>= (string-length s) w)
      (string-append s " ")
      (string-append s (make-spaces (- w (string-length s))))))

;; "C-x C-f" / "RET (dired-mode-map)" / "M-x name" when unbound.
(define (help-binding-string name)
  (let ((bs (command-bindings name)))
    (if (null? bs)
        (string-append "M-x " name)
        (string-join
         (map (lambda (b)
                (if (equal? (list-ref b 1) "")
                    (car b)
                    (string-append (car b) " (" (list-ref b 1) ")")))
              bs)
         ", "))))

(define (help-command-text name)
  (let ((doc (command-doc name)))
    (string-append
     name " is an interactive command.\n\n"
     "It is bound to: " (help-binding-string name) "\n\n"
     "Documentation:\n"
     (if (equal? doc "") "Not documented." doc)
     "\n")))

;; ---- C-h c / C-h k ---------------------------------------------------------------

(define (describe-key-briefly)
  (read-key-sequence "Describe key briefly: "
    (lambda (seq name)
      (if (equal? name "")
          (message (string-append seq " is undefined"))
          (message (string-append seq " runs the command " name))))))

(define (describe-key)
  (read-key-sequence "Describe key: "
    (lambda (seq name)
      (if (equal? name "")
          (message (string-append seq " is undefined"))
          (help-display
           (string-append seq " runs the command " name "\n\n"
                          (help-command-text name)))))))

;; ---- C-h x / C-h f ---------------------------------------------------------------

(define (describe-command)
  (read-string "Describe command: " "" "command"
    (lambda (name)
      (if (member name (command-names))
          (help-display (help-command-text name))
          (message (string-append "No such command: " name))))))

;; ---- C-h v -----------------------------------------------------------------------

;; taco "variables" reachable from here: the current buffer's buffer-local
;; variables (buffer-local-set!) and the native editor options
;; (set-option). Scheme globals like indent-width live inside the
;; interpreter and can't be enumerated from outside it.
(define *help-known-options* '("display-line-numbers"))

(define (help-value->string v)
  (cond ((equal? v #t) "#t")
        ((equal? v #f) "#f")
        ((string? v) (string-append "\"" v "\""))
        ((number? v) (number->string v))
        ((list? v) (string-append "(" (string-join (map help-value->string v) " ") ")"))
        (else "#<value>")))

(define (describe-variable)
  (read-string "Describe variable: " "" ""
    (lambda (name)
      (cond ((member name (buffer-local-keys))
             (help-display
              (string-append
               name " is a buffer-local variable in buffer " (current-buffer) ".\n\n"
               "Value: " (help-value->string (buffer-local-get name)) "\n")))
            ((member name *help-known-options*)
             (help-display
              (string-append
               name " is an editor option (set-option).\n\n"
               "Value: " (help-value->string (get-option name)) "\n")))
            (else
             (message (string-append name
                                     " is not defined as a variable (buffer-local or option)")))))))

;; ---- C-h a -----------------------------------------------------------------------

(define (command-apropos)
  (read-string "Command apropos (regexp): " "" ""
    (lambda (pattern)
      (let ((hits (filter (lambda (n) (regexp-match? pattern n)) (command-names))))
        (if (null? hits)
            (message (string-append "No commands matching " pattern))
            (help-display
             (apply string-append
                    (string-append "Commands matching \"" pattern "\":\n\n")
                    (map (lambda (n)
                           (let ((doc (command-doc n)))
                             (string-append
                              (help-pad n 36) (help-binding-string n) "\n"
                              (if (equal? doc "") "" (string-append "    " doc "\n")))))
                         hits))))))))

;; ---- C-h ? -----------------------------------------------------------------------

(define (help-for-help)
  (help-display
   (string-append
    "You have typed C-h, the help character. Help options:\n"
    "\n"
    "C-h c KEYS      Describe key briefly: which command a key sequence runs.\n"
    "C-h k KEYS      Describe key: full documentation of that command.\n"
    "C-h x COMMAND   Describe a command by name (C-h f is an alias).\n"
    "C-h v VARIABLE  Describe a variable (buffer-locals and options).\n"
    "C-h a REGEXP    Command apropos: list commands matching a regexp,\n"
    "                with the keys that run them.\n"
    "C-h ?           This overview.\n"
    "\n"
    "Dismiss this window with C-x 1, or q inside it. C-g cancels a\n"
    "pending C-h. M-x help RET shows this overview too.\n")))

;; ---- commands & keymap -------------------------------------------------------------

(define-command "describe-key-briefly"
  "Echo which command a key sequence runs."
  describe-key-briefly)
(define-command "describe-key"
  "Show the full documentation of the command a key sequence runs."
  describe-key)
(define-command "describe-command"
  "Show the full documentation of a command, by name."
  describe-command)
(define-command "describe-function"
  "Show the full documentation of a command, by name (alias of describe-command)."
  describe-command)
(define-command "describe-variable"
  "Show the value of a buffer-local variable or editor option."
  describe-variable)
(define-command "command-apropos"
  "List all commands whose names match a regexp, with their key bindings."
  command-apropos)
(define-command "help-for-help"
  "Show an overview of the C-h help commands."
  help-for-help)
(define-command "help"
  "Show an overview of the C-h help commands."
  help-for-help)

(global-set-key "C-h c" "describe-key-briefly")
(global-set-key "C-h k" "describe-key")
(global-set-key "C-h x" "describe-command")
(global-set-key "C-h f" "describe-function")
(global-set-key "C-h v" "describe-variable")
(global-set-key "C-h a" "command-apropos")
(global-set-key "C-h ?" "help-for-help")

(define-key "help-mode-map" "q" "quit-window")
