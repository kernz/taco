;; man.scm — M-x man, entirely in Steel, mirroring Emacs' man.el on taco's
;; contract: run `man TOPIC` asynchronously (start-process), catch its
;; nroff backspace-overstrike output (GROFF_NO_SGR / MAN_KEEP_FORMATTING
;; force the classic c BS c = bold, _ BS c = underline encoding even into
;; a pipe), strip the overstrikes into clean text plus face spans, and show
;; the result read-only in a "*Man TOPIC*" buffer in the other window
;; without stealing focus (Emacs' Man-notify-method 'friendly). Rust knows
;; nothing about "man" — the one primitive added for this file is
;; (window-width), which MANWIDTH is derived from, exactly as in Emacs.
;;
;; Buffer-local state of a *Man ...* buffer:
;;   "man-raw"   — the raw overstrike output accumulated by the filter
;;                 (the process buffer text itself is a scratch area the
;;                 final render overwrites, so a rerun over a failed
;;                 buffer can't contaminate the parse)
;;   "man-proc"  — live process id while formatting
;;   "man-ready" — #t once rendered; a repeat M-x man on the same topic
;;                 just redisplays the buffer (Emacs reuses it too)

(set-face-color "Man-overstrike" "yellow")
(set-face-color "Man-underline" "cyan")

;; ---- overstrike fontifying ----------------------------------------------------

(define man-backspace (string (integer->char 8)))

;; (pieces . last-char) with the final character removed. `pieces` is the
;; clean text so far as a reversed list of non-empty chunks.
(define (man-pop-last pieces)
  (let* ((h (car pieces))
         (l (string-length h))
         (ch (substring h (- l 1) l))
         (rest (substring h 0 (- l 1))))
    (cons (if (equal? rest "") (cdr pieces) (cons rest (cdr pieces))) ch)))

;; Extend the newest span by one char when this overstrike continues the
;; same face run (bold words arrive as N BS N a BS a ... — one span each
;; would make render's per-line sweep quadratic on a big page).
(define (man-span-add spans pos face)
  (cond ((equal? face #f) spans)
        ((and (pair? spans)
              (equal? (list-ref (car spans) 1) pos)
              (equal? (list-ref (car spans) 2) face))
         (cons (list (car (car spans)) (+ pos 1) face) (cdr spans)))
        (else (cons (list pos (+ pos 1) face) spans))))

;; nroff output -> (clean-text (start end face) ...). Each backspace pairs
;; the char before it with the char after it: equal chars mean bold
;; (Man-overstrike), an underscore on either side means underline
;; (Man-underline), keeping the letter rather than the underscore; any
;; other pair keeps the later char, unfontified.
(define (man-fontify-parse raw)
  (let loop ((ps (string-split-char raw man-backspace))
             (pieces '()) (len 0) (spans '()))
    (if (null? ps)
        (cons (apply string-append (reverse pieces)) (reverse spans))
        (let ((p (car ps)))
          (cond
           ;; The leading chunk, or a backspace with nothing left before it.
           ((= len 0)
            (loop (cdr ps)
                  (if (equal? p "") pieces (cons p pieces))
                  (string-length p) spans))
           ;; "x BS BS y" or a trailing backspace: drop the overstruck char.
           ((equal? p "")
            (loop (cdr ps) (car (man-pop-last pieces)) (- len 1) spans))
           (else
            (let* ((popped (man-pop-last pieces))
                   (a (cdr popped))
                   (b (substring p 0 1))
                   (face (cond ((equal? a b) "Man-overstrike")
                               ((equal? a "_") "Man-underline")
                               ((equal? b "_") "Man-underline")
                               (else #f)))
                   (ch (if (equal? b "_") a b)))
              (loop (cdr ps)
                    (cons (string-append ch (substring p 1 (string-length p)))
                          (car popped))
                    (+ len (- (string-length p) 1))
                    (man-span-add spans (- len 1) face)))))))))

;; ---- topic parsing --------------------------------------------------------------

;; Emacs' Man-translate-references: "ls(2)" -> "2 ls"; anything else is
;; passed to man verbatim (so "2 ls" and switches like "-k foo" work).
(define (man-translate-topic topic)
  (let ((m (regexp-match "^([^ ()]+)\\(([0-9][0-9a-zA-Z]*|[a-zA-Z])\\)$" topic)))
    (if m
        (string-append (list-ref m 2) " " (list-ref m 1))
        topic)))

;; ---- topic completion ------------------------------------------------------------
;; The man prompt carries completion kind "man"; a completion UI (see
;; examples/vertico.scm) asks (man-topic-names) for the candidates —
;; every page known to apropos, as "name(section)", built once per
;; session from `man -k .` (Emacs' Man-completion-table caches the same
;; way). No completion plugin loaded = plain prompt, like everywhere else.

(define *man-topics* #f)

;; awk turns each apropos line "name[, name2, ...] (sec) - description"
;; into one "name(sec)" line per name (unparsable lines print nothing),
;; sorted shortest-then-alphabetical — vertico's default candidate order,
;; which filter-matching preserves within its prefix/substring groups, so
;; typing "printf" ranks printf(1) above printf_function(3type). The
;; parse lives in the shell pipeline (and the split in the native
;; string-lines) because walking ~9000 lines through the interpreter
;; takes minutes.
(define man-apropos-command
  (string-append
   "man -k . 2>/dev/null | awk -F' \\\\(' "
   "'NF>1 {sec=$2; sub(/\\).*/,\"\",sec); n=split($1,a,\", \"); "
   "for(i=1;i<=n;i++) print a[i]\"(\"sec\")\"}'"
   " | awk '{print length, $0}' | sort -k1,1n -k2 | cut -d' ' -f2-"))

(define (man-topic-names)
  (unless *man-topics*
    (set! *man-topics* (string-lines (car (run-shell-command man-apropos-command)))))
  *man-topics*)

;; Candidates for the man prompt. Plain input matches whole "name(sec)"
;; entries; Emacs' "SEC NAME" form ("3 mal") completes names within that
;; section. Either way the candidates keep the "name(sec)" spelling —
;; submitting one goes through man-translate-topic, which accepts it.
(define (man-completion-candidates input)
  (let ((m (regexp-match "^([0-9][0-9a-zA-Z]*|[a-zA-Z]) +(.*)$" input)))
    (if m
        (filter-matching
         (filter-suffix (man-topic-names)
                        (string-append "(" (list-ref m 1) ")"))
         (list-ref m 2))
        (filter-matching (man-topic-names) input))))

(define (man-word-char? s) (regexp-match? "^[-A-Za-z0-9_.:+@]$" s))

;; Emacs' Man-default-entry: the topic-looking word around point, keeping a
;; "(3)"-style section suffix sitting right after it — what RET on a
;; "printf(3)" reference in SEE ALSO follows.
(define (man-default-entry)
  (let* ((lines (buffer-lines))
         (idx (- (line-number) 1))
         (line (list-ref lines idx))
         (len (string-length line))
         (col (min (- (point) (line-start-offset lines idx)) len))
         (start (let loop ((i col))
                  (if (and (> i 0) (man-word-char? (substring line (- i 1) i)))
                      (loop (- i 1))
                      i)))
         (end (let loop ((i col))
                (if (and (< i len) (man-word-char? (substring line i (+ i 1))))
                    (loop (+ i 1))
                    i))))
    (if (= start end)
        ""
        (let* ((word (substring line start end))
               (rest (substring line end len))
               (m (regexp-match "^\\(([0-9][0-9a-zA-Z]*|[a-zA-Z])\\)" rest)))
          (if m (string-append word (car m)) word)))))

;; ---- fetching and displaying a page -----------------------------------------------

;; Emacs' Man-width-max: format for the window, but never wider than this
;; many columns — in a wide frame the page keeps a readable measure and
;; the window's right side stays blank, instead of stretching edge to edge.
(define Man-width-max 80)

(define (man-first-line s) (car (string-split-char s "\n")))

;; Render the accumulated raw output into `bufname` and show it in the
;; other window without selecting it (Man-notify-method 'friendly).
(define (man-render-and-show bufname raw)
  (other-window-or-split)
  (switch-to-buffer bufname)
  (let ((parsed (man-fontify-parse raw)))
    (set-buffer-string! (car parsed))       ; also clears old face spans
    (for-each
     (lambda (sp)
       (buffer-add-face-span! bufname (car sp) (list-ref sp 1) (list-ref sp 2)))
     (cdr parsed)))
  (goto-char 0)
  (set-buffer-read-only! #t)
  (set-buffer-mode-name "Man")
  (use-local-map "Man-mode-map")
  (buffer-local-set! "man-ready" #t)
  (other-window))

(define (man-redisplay bufname)
  (unless (equal? (current-buffer) bufname)
    (other-window-or-split)
    (switch-to-buffer bufname)
    (other-window)))

(define (man-getpage topic)
  ;; The buffer is named after the translated reference, so "malloc(3)"
  ;; and "3 malloc" share one "*Man 3 malloc*" buffer (as in Emacs).
  (let ((bufname (string-append "*Man " (man-translate-topic topic) "*")))
    (cond
     ;; Already formatted: just show it again (Emacs reuses the buffer).
     ((equal? (buffer-local-get-in bufname "man-ready") #t)
      (man-redisplay bufname))
     ((let ((p (buffer-local-get-in bufname "man-proc")))
        (and (not (equal? p #f)) (process-live? p)))
      (message (string-append "Already formatting " bufname)))
     (else
      (let* ((args (man-translate-topic topic))
             ;; MANWIDTH makes man format for our window (capped at
             ;; Man-width-max, as in Emacs); the other two force
             ;; overstrike output into the pipe (see file header).
             (cmd (string-append
                   "MANWIDTH=" (number->string
                                (max 30 (min Man-width-max (- (window-width) 1))))
                   " GROFF_NO_SGR=1 MAN_KEEP_FORMATTING=1 man " args))
             (id (start-process "man" bufname ""
                   cmd
                   ;; stdout+stderr chunks; the pump appended them to the
                   ;; buffer already, but the parse reads this accumulator
                   ;; so stale text from a failed earlier run can't leak in.
                   (lambda (text start end)
                     (buffer-local-set-in! bufname "man-raw"
                       (string-append (buffer-local-get-in bufname "man-raw") text)))
                   (lambda (code)
                     (let ((raw (buffer-local-get-in bufname "man-raw")))
                       (if (equal? code 0)
                           (man-render-and-show bufname raw)
                           (message (let ((l (man-first-line raw)))
                                      (if (equal? l "") "man: exited abnormally" l)))))))))
        (unless (< id 0)
          (buffer-local-set-in! bufname "man-raw" "")
          (buffer-local-set-in! bufname "man-ready" #f)
          (buffer-local-set-in! bufname "man-proc" id)
          (message (string-append "Invoking man " args " in the background"))))))))

;; ---- section motion ------------------------------------------------------------

;; Emacs' Man-heading-regexp: an all-caps line starting in column 0.
(define (man-heading? line) (regexp-match? "^[A-Z][A-Z0-9 /-]*$" line))

;; 1-based line numbers of the section headings.
(define (man-section-lines)
  (let loop ((ls (buffer-lines)) (n 1) (acc '()))
    (if (null? ls)
        (reverse acc)
        (loop (cdr ls) (+ n 1) (if (man-heading? (car ls)) (cons n acc) acc)))))

(define (Man-next-section)
  (let ((next (filter (lambda (n) (> n (line-number))) (man-section-lines))))
    (if (null? next)
        (message "No next section")
        (goto-line (car next)))))

(define (Man-previous-section)
  (let ((prev (filter (lambda (n) (< n (line-number))) (man-section-lines))))
    (if (null? prev)
        (goto-line 1)
        (goto-line (car (reverse prev))))))

(define (man-goto-heading name)
  (let loop ((ls (buffer-lines)) (n 1))
    (cond ((null? ls) (message (string-append "No section " name)))
          ((and (man-heading? (car ls))
                (regexp-match? (string-append "(?i)^" name) (car ls)))
           (goto-line n))
          (else (loop (cdr ls) (+ n 1))))))

(define (Man-goto-section)
  (read-string "Go to section: " "" "" man-goto-heading))

(define (Man-goto-see-also-section) (man-goto-heading "SEE ALSO"))

;; ---- commands ----------------------------------------------------------------

;; Emacs' prompt shape: the reference at point rides along as a default
;; ("Manual entry (default printf(3)): ") rather than as pre-filled input,
;; so a completion UI starts from the full candidate list.
(define (man-read-entry)
  (let ((default (man-default-entry)))
    (read-string (if (equal? default "")
                     "Manual entry: "
                     (string-append "Manual entry (default " default "): "))
                 "" "man"
      (lambda (topic)
        (let ((topic (if (equal? topic "") default topic)))
          (if (equal? topic "")
              (message "No man args given")
              (man-getpage topic)))))))

(define (man) (man-read-entry))

;; RET on a "printf(3)" reference: follow it without prompting.
(define (man-follow)
  (let ((topic (man-default-entry)))
    (if (equal? topic "")
        (message "No item under point")
        (man-getpage topic))))

;; r: like man-follow but through the prompt, so the reference is editable.
(define (Man-follow-manual-page) (man-read-entry))

(define (Man-kill)
  (kill-buffer)
  (delete-window))

(define-command "man"
  "Read a man page of a topic (\"ls\", \"2 open\", \"printf(3)\") into a *Man* buffer."
  man)
(define-command "man-follow"
  "Read the man page for the topic at point."
  man-follow)
(define-command "Man-follow-manual-page"
  "Prompt (default: the reference at point) and read that man page."
  Man-follow-manual-page)
(define-command "Man-next-section"
  "Move point to the next section heading of this man page."
  Man-next-section)
(define-command "Man-previous-section"
  "Move point to the previous section heading of this man page."
  Man-previous-section)
(define-command "Man-goto-section"
  "Move point to a section of this man page, by name."
  Man-goto-section)
(define-command "Man-goto-see-also-section"
  "Move point to the SEE ALSO section of this man page."
  Man-goto-see-also-section)
(define-command "Man-kill"
  "Kill this *Man* buffer and delete its window."
  Man-kill)

;; ---- keymap -------------------------------------------------------------------

(define-key "Man-mode-map" "RET" "man-follow")
(define-key "Man-mode-map" "r"   "Man-follow-manual-page")
(define-key "Man-mode-map" "n"   "Man-next-section")
(define-key "Man-mode-map" "p"   "Man-previous-section")
(define-key "Man-mode-map" "g"   "Man-goto-section")
(define-key "Man-mode-map" "s"   "Man-goto-see-also-section")
(define-key "Man-mode-map" "m"   "man")
(define-key "Man-mode-map" "k"   "Man-kill")
(define-key "Man-mode-map" "q"   "quit-window")
(define-key "Man-mode-map" "SPC" "scroll-up-command")
(define-key "Man-mode-map" "backspace" "scroll-down-command")
