;; taco bootstrap — the default keymap, built entirely through the public
;; Scheme contract. User config (~/.config/taco/init.scm) runs after this
;; and can rebind anything with the same functions.

;; ---- Appearance ---------------------------------------------------------
(set-face-color "mode-line" "blue")
(set-face-color "highlight" "white")

;; ---- Basics & system --------------------------------------------------
(global-set-key "C-x C-c" "save-buffers-kill-terminal")
(global-set-key "C-x C-s" "save-buffer")
(global-set-key "C-x b"   "switch-to-buffer")
(global-set-key "C-x k"   "kill-buffer")
(global-set-key "C-x C-f" "find-file")
(global-set-key "C-x C-j" "dired-jump")
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
(global-set-key "C-h k" "describe-key")
(global-set-key "C-h f" "describe-function")

;; ---- Dired entry points -----------------------------------------------------
(global-set-key "C-c f d" "dired-open-dir")
(global-set-key "C-c o -" "dired-current")
(global-set-key "C-c p D" "dired-project-root")

;; ---- Dired mode map -----------------------------------------------------------
(dired-set-key "RET" "dired-find-file")
(dired-set-key "o"   "dired-find-file-other-window")
(dired-set-key "^"   "dired-up-directory")
(dired-set-key "m"   "dired-mark")
(dired-set-key "% m" "dired-mark-regexp")
(dired-set-key "!"   "dired-shell-command")
(dired-set-key "d"   "dired-flag-deletion")
(dired-set-key "x"   "dired-do-flagged-delete")
(dired-set-key "u"   "dired-unmark")
(dired-set-key "U"   "dired-unmark-all")
(dired-set-key "D"   "dired-do-delete")
(dired-set-key "R"   "dired-do-rename")
(dired-set-key "C"   "dired-do-copy")
(dired-set-key "+"   "dired-create-directory")
(dired-set-key "="   "dired-diff")
(dired-set-key "Z"   "dired-compress")
(dired-set-key "g"   "dired-revert")
(dired-set-key ")"   "dired-toggle-hidden")
(dired-set-key "q"   "dired-kill-all")
(dired-set-key "C-c C-e" "wgrep-mode")

;; ---- Wgrep (writable dired) map --------------------------------------------------
(wgrep-set-key "C-c C-c" "wgrep-commit")
(wgrep-set-key "C-c C-k" "wgrep-abort")

;; ---- Hooks -----------------------------------------------------------------------
;; Emacs-style named hooks. The native core fires, per input event:
;;   "minibuffer-setup-hook"  — a prompt just opened
;;   "post-command-hook"      — a key was handled while a prompt is active
;;   "minibuffer-exit-hook"   — the prompt closed (RET, C-g, exit-minibuffer)
;; Completion UIs (see examples/vertico.scm) hang off these together with
;; (minibuffer-contents), (minibuffer-completion-kind), (minibuffer-set-key)
;; and (minibuffer-show-candidates). Unlike Emacs, (remove-hook name) takes
;; no function argument: it removes every function on the named hook.

(define *hooks* '())

;; (fn is Steel's lambda alias, hence the `f` parameter name)
(define (add-hook name f)
  (set! *hooks* (cons (cons name f) *hooks*)))

(define (remove-hook name)
  (set! *hooks* (filter (lambda (h) (not (equal? (car h) name))) *hooks*)))

(define (run-hooks name)
  (for-each (lambda (h) (when (equal? (car h) name) ((cdr h))))
            *hooks*))
