;; vertico.scm — a Vertico-style completion UI for taco, as a user plugin.
;;
;; Install: copy this whole file into ~/.config/taco/init.scm (or append it
;; to your existing config). It is pure Scheme over the public contract —
;; nothing here is wired into the editor core.
;;
;; What it does, once loaded:
;;   * Every completable prompt (M-x, C-x C-f, C-x b, C-h f, dired paths)
;;     shows its candidates vertically under the prompt, up to 6 rows,
;;     refreshed on every keystroke. The selected candidate is highlighted.
;;   * C-n / C-p cycle the selection (wrapping at either end).
;;   * RET submits the selected candidate (or your literal input when
;;     nothing matches).
;;   * TAB inserts the selected candidate into the prompt without
;;     submitting — on a directory candidate this descends into it.
;;
;; It hangs off the minibuffer hooks fired by the core
;; (minibuffer-setup-hook / post-command-hook / minibuffer-exit-hook), reads
;; the prompt through (minibuffer-contents) / (minibuffer-completion-kind),
;; and drives the native candidate list with (minibuffer-show-candidates).

;; ---- State ---------------------------------------------------------------

(define vertico--matches '())      ;; candidates currently displayed
(define vertico--index 0)          ;; selected candidate
(define vertico--last-input #f)    ;; input at the last refresh (#f = force)
(define vertico--last-prompt "")   ;; detects prompt->prompt transitions

;; ---- Small string/list helpers --------------------------------------------

(define (vertico--prefix? p s)
  (and (<= (string-length p) (string-length s))
       (equal? p (substring s 0 (string-length p)))))

(define (vertico--substring? needle hay)
  (let ((nl (string-length needle))
        (hl (string-length hay)))
    (let loop ((i 0))
      (cond ((> (+ i nl) hl) #f)
            ((equal? needle (substring hay i (+ i nl))) #t)
            (else (loop (+ i 1)))))))

(define (vertico--nth lst i)
  (if (= i 0) (car lst) (vertico--nth (cdr lst) (- i 1))))

;; Directory candidates carry a trailing "/" (see directory-files).
(define (vertico--directory? name)
  (let ((l (string-length name)))
    (and (> l 0) (equal? "/" (substring name (- l 1) l)))))

;; Directories ahead of files, each side keeping its alphabetical order.
(define (vertico--dirs-first names)
  (append (filter vertico--directory? names)
          (filter (lambda (n) (not (vertico--directory? n))) names)))

;; "dir/par" -> "dir/",  "par" -> ""
(define (vertico--dir-part s)
  (let loop ((i (- (string-length s) 1)))
    (cond ((< i 0) "")
          ((equal? "/" (substring s i (+ i 1))) (substring s 0 (+ i 1)))
          (else (loop (- i 1))))))

;; ---- Matching --------------------------------------------------------------
;; Adapted from vertico's default completion styles: prefix matches first,
;; then substring matches, both in the source's (sorted) order.

(define (vertico--matching candidates input)
  (if (equal? input "")
      candidates
      (append
       (filter (lambda (c) (vertico--prefix? input c)) candidates)
       (filter (lambda (c) (and (not (vertico--prefix? input c))
                                (vertico--substring? input c)))
               candidates))))

(define (vertico--compute input kind)
  (cond ((equal? kind "command") (vertico--matching (command-names) input))
        ((equal? kind "buffer")  (vertico--matching (buffer-names) input))
        ((equal? kind "file")
         (let* ((dir (vertico--dir-part input))
                (part (substring input (string-length dir) (string-length input))))
           (vertico--matching
            (vertico--dirs-first
             (directory-files (if (equal? dir "") (default-directory) dir)))
            part)))
        (else '())))

;; The full prompt text a candidate stands for: file candidates are names
;; inside the directory part already typed.

(define (vertico--expansion name)
  (if (equal? (minibuffer-completion-kind) "file")
      (string-append (vertico--dir-part (minibuffer-contents)) name)
      name))

;; ---- Refresh (driven by the hooks) -------------------------------------------

(define (vertico--show)
  (minibuffer-show-candidates vertico--matches vertico--index))

(define (vertico--reset)
  (set! vertico--matches '())
  (set! vertico--index 0)
  (set! vertico--last-input #f)
  (set! vertico--last-prompt (minibuffer-prompt)))

(define (vertico--refresh)
  (when (minibufferp)
    ;; A prompt replaced another prompt (e.g. query-replace's second
    ;; question) surfaces as post-command-hook; start over.
    (when (not (equal? (minibuffer-prompt) vertico--last-prompt))
      (vertico--reset))
    (let ((kind (minibuffer-completion-kind))
          (input (minibuffer-contents)))
      (when (and (not (equal? kind ""))
                 (not (equal? input vertico--last-input)))
        (set! vertico--last-input input)
        (set! vertico--matches (vertico--compute input kind))
        (set! vertico--index 0)
        (vertico--show)))))

(add-hook "minibuffer-setup-hook" (lambda () (vertico--reset) (vertico--refresh)))
(add-hook "post-command-hook" vertico--refresh)
(add-hook "minibuffer-exit-hook" vertico--reset)

;; ---- Commands & keys ------------------------------------------------------------

(define (vertico--move step)
  (let ((n (length vertico--matches)))
    (when (> n 0)
      (set! vertico--index (modulo (+ vertico--index step n) n))
      (vertico--show))))

(define-command "vertico-next" "Select the next completion candidate"
  (lambda () (vertico--move 1)))

(define-command "vertico-previous" "Select the previous completion candidate"
  (lambda () (vertico--move -1)))

(define-command "vertico-exit" "Submit the selected candidate (or the literal input)"
  (lambda ()
    (when (not (null? vertico--matches))
      (set-minibuffer-contents
       (vertico--expansion (vertico--nth vertico--matches vertico--index))))
    (exit-minibuffer)))

(define-command "vertico-insert" "Insert the selected candidate into the prompt"
  (lambda ()
    (when (not (null? vertico--matches))
      (set-minibuffer-contents
       (vertico--expansion (vertico--nth vertico--matches vertico--index))))))

(minibuffer-set-key "C-n" "vertico-next")
(minibuffer-set-key "C-p" "vertico-previous")
(minibuffer-set-key "RET" "vertico-exit")
(minibuffer-set-key "TAB" "vertico-insert")
