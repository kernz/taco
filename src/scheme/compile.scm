;; compile.scm — M-x compile, entirely in Steel: run a build command
;; asynchronously (start-process, the Rust async primitive), stream its
;; output into a read-only *compilation* buffer, recognize error messages
;; as they arrive, color them, and jump to them (RET here, next-error
;; anywhere). Mirrors Emacs' compile.el on taco's contract; Rust knows
;; nothing about "compilation".
;;
;; The parse/jump core is deliberately split out under the `results-`
;; prefix and keyed by buffer-locals, so any buffer full of
;; "file:line[:col]" lines can opt in (dired's A command builds a
;; grep-style buffer on exactly this machinery — see dired.scm).
;;
;; Buffer-local state of a results buffer:
;;   "compilation-errors"     — records (msg-line file line col type),
;;                              newest first (see results-errors)
;;   "compilation-current"    — index of the last visited record, -1
;;   "compilation-dir"        — directory relative file names resolve from
;;   "compilation-command"    — for g (recompile)
;;   "compilation-process"    — live process id (compile buffers only)
;;   "compilation-generation" — guards stale filter/sentinel callbacks
;;                              after a recompile killed their process
;;   "compilation-partial"    — unterminated tail of the last chunk
;;   "compilation-partial-offset" — its char offset in the buffer
;;   "compilation-line"       — 1-based count of complete lines parsed
;;   "compilation-last-type"  — severity context (a rustc "error[E…]:"
;;                              header line colors its later "-->" line)

;; ---- error message rules ----------------------------------------------------
;; (name pattern file-group line-group col-group type); groups are regex
;; capture indices, col-group/type may be #f (#f type = sniff severity from
;; the line text, falling back to the message-context severity). First
;; match wins, so specific rules come before the generic `gnu` one.
;; Extend from init.scm with (add-compilation-error-regexp entry).

(define *compilation-error-regexps*
  (list
   ;; rustc/clippy: "  --> src/main.rs:12:5" (severity on the message line
   ;; above it, tracked as compilation-last-type).
   (list "rust" "^\\s*-->\\s+([^:\\s]+):([0-9]+):([0-9]+)" 1 2 3 #f)
   ;; rustc 1.65+ panic: "thread 'main' panicked at src/main.rs:3:5:"
   (list "rust-panic" "panicked at ([^:\\s]+):([0-9]+):([0-9]+)" 1 2 3 "error")
   ;; Python traceback: "  File \"x.py\", line 3, in <module>"
   (list "python" "^\\s*File \"([^\"]+)\", line ([0-9]+)" 1 2 #f "error")
   ;; Java/JVM stack frame: "\tat pkg.Cls.m(Cls.java:42)"
   (list "java" "^\\s+at [^(]+\\(([^:()]+):([0-9]+)\\)" 1 2 #f "info")
   ;; gcc/clang include chains: "In file included from x.c:10," / "  from y.h:2,"
   (list "gcc-include" "^(?:In file included from|\\s+from) ([^,:\\s]+):([0-9]+)" 1 2 #f "info")
   ;; GNU make: "make: *** [Makefile:5: target] Error 1"
   (list "gmake" ": \\*\\*\\* \\[([^:\\]]+):([0-9]+)" 1 2 #f "info")
   ;; The universal GNU "file:line[:col]:" form, last so everything above
   ;; wins first. The file may not contain spaces — that restriction is
   ;; what keeps time stamps ("… at Sun Jul 13 12:34:56") from matching.
   (list "gnu" "^([^ :\n]+):([0-9]+):(?:([0-9]+):)?" 1 2 3 #f)))

(define (add-compilation-error-regexp entry)
  (set! *compilation-error-regexps* (cons entry *compilation-error-regexps*)))

(set-face-color "compilation-error" "red")
(set-face-color "compilation-warning" "yellow")
(set-face-color "compilation-info" "green")

;; ---- shared results machinery ------------------------------------------------

(define *compile-command* "make -k ")
(define *next-error-buffer* #f)         ; Emacs' next-error-last-buffer

;; buffer-local-get-in with a default for missing keys.
(define (results-local bufname key default)
  (let ((v (buffer-local-get-in bufname key)))
    (if (equal? v #f) default v)))

(define (results-errors bufname)
  (reverse (results-local bufname "compilation-errors" '())))

(define (results-count-lines s)
  (- (length (string-split-char s "\n")) 1))

(define (results-normalize-type t)
  (if (equal? t "note") "info" t))

;; Severity of a matched line: explicit words in the line itself, else the
;; context set by the last message-header line, else "error".
(define (results-sniff-type bufname line)
  (cond ((regexp-match? "[Ww]arning" line) "warning")
        ((regexp-match? "[Nn]ote:|[Ii]nfo" line) "info")
        ((regexp-match? "[Ee]rror" line) "error")
        (else (results-local bufname "compilation-last-type" "error"))))

;; The substring of `line` for capture group `idx` of a
;; regexp-match-positions result, or #f when absent.
(define (results-group-string line pos idx)
  (if (or (equal? idx #f)
          (>= idx (length pos))
          (equal? (list-ref pos idx) #f))
      #f
      (let ((p (list-ref pos idx)))
        (substring line (car p) (list-ref p 1)))))

(define (results-record! bufname rule pos line line-no offset)
  (let* ((file (results-group-string line pos (list-ref rule 2)))
         (lstr (results-group-string line pos (list-ref rule 3)))
         (cstr (results-group-string line pos (list-ref rule 4)))
         (type (let ((t (list-ref rule 5)))
                 (if t t (results-sniff-type bufname line))))
         (whole (list-ref pos 0)))
    (when (and file lstr)
      (buffer-local-set-in! bufname "compilation-errors"
        (cons (list line-no file (string->number lstr)
                    (if cstr (string->number cstr) #f) type)
              (results-local bufname "compilation-errors" '())))
      (buffer-add-face-span! bufname
                             (+ offset (car whole))
                             (+ offset (list-ref whole 1))
                             (string-append "compilation-" type)))))

;; Parse one complete buffer line sitting at char `offset`, buffer line
;; number `line-no` (1-based).
(define (results-parse-line bufname line line-no offset)
  ;; A rustc message header ("error[E0308]: …", "warning: …") sets the
  ;; severity its later "-->" location line inherits.
  (let ((sev (regexp-match "^(error|warning|note)[:\\[]" line)))
    (when sev
      (buffer-local-set-in! bufname "compilation-last-type"
                            (results-normalize-type (list-ref sev 1)))))
  (let loop ((rules *compilation-error-regexps*))
    (unless (null? rules)
      (let ((pos (regexp-match-positions (list-ref (car rules) 1) line 0)))
        (if (equal? pos #f)
            (loop (cdr rules))
            (results-record! bufname (car rules) pos line line-no offset))))))

;; Parse the *current* buffer as a fresh results buffer (a grep-style
;; listing built synchronously — dired's A). The async compile path parses
;; incrementally instead (compilation-filter).
(define (results-parse-current-buffer! dir)
  (let ((bufname (current-buffer)))
    (buffer-clear-face-spans! bufname)
    (buffer-local-set! "compilation-errors" '())
    (buffer-local-set! "compilation-current" -1)
    (buffer-local-set! "compilation-dir" dir)
    (buffer-local-set! "compilation-last-type" "error")
    (set! *next-error-buffer* bufname)
    (let loop ((ls (buffer-lines)) (ln 1) (o 0))
      (unless (null? ls)
        (results-parse-line bufname (car ls) ln o)
        (loop (cdr ls) (+ ln 1) (+ o (string-length (car ls)) 1))))))

;; ---- jumping -------------------------------------------------------------------

(define (results-resolve-file dir file)
  (if (string-starts-with? file "/")
      file
      (string-append dir (if (string-ends-with? dir "/") "" "/") file)))

;; Visit the source position of record `idx` (into the ordered list).
(define (results-jump bufname idx)
  (let* ((errs (results-errors bufname))
         (n (length errs)))
    (cond ((null? errs) (message "No errors to jump to"))
          ((< idx 0) (message "Moved back before first error"))
          ((>= idx n) (message "No more errors"))
          (else
           (buffer-local-set-in! bufname "compilation-current" idx)
           (set! *next-error-buffer* bufname)
           (let* ((r (list-ref errs idx))
                  (file (list-ref r 1))
                  (line (list-ref r 2))
                  (col (list-ref r 3))
                  (dir (results-local bufname "compilation-dir" "")))
             ;; From inside the results buffer, show the source in the
             ;; other window (Emacs' compile-goto-error); from anywhere
             ;; else (next-error), reuse the current window.
             (when (equal? (current-buffer) bufname)
               (other-window-or-split))
             (find-file (results-resolve-file dir file))
             (goto-line line)
             (when col
               (goto-char (+ (point) (max 0 (- col 1))))))))))

;; Index of the last record on-or-before buffer line `ln`, or #f.
(define (results-index-at-or-before errs ln)
  (let loop ((es errs) (i 0) (best #f))
    (cond ((null? es) best)
          ((<= (car (car es)) ln) (loop (cdr es) (+ i 1) i))
          (else best))))

;; Index of the first record strictly after buffer line `ln`, or #f.
(define (results-index-after errs ln)
  (let loop ((es errs) (i 0))
    (cond ((null? es) #f)
          ((> (car (car es)) ln) i)
          (else (loop (cdr es) (+ i 1))))))

;; Index of the last record strictly before buffer line `ln`, or #f.
(define (results-index-before errs ln)
  (let loop ((es errs) (i 0) (best #f))
    (cond ((null? es) best)
          ((< (car (car es)) ln) (loop (cdr es) (+ i 1) i))
          (else best))))

;; RET on a message line: visit its source position.
(define (compile-goto-error)
  (let* ((bufname (current-buffer))
         (errs (results-errors bufname)))
    (if (null? errs)
        (message "No errors here")
        (let ((idx (results-index-at-or-before errs (line-number))))
          (if idx
              (results-jump bufname idx)
              (message "No error on this line"))))))

;; n/p inside the results buffer: move point between message lines without
;; visiting the source (visiting is RET / next-error).
(define (compilation-next-error-no-select)
  (let* ((bufname (current-buffer))
         (errs (results-errors bufname))
         (idx (results-index-after errs (line-number))))
    (if idx
        (begin
          (set! *next-error-buffer* bufname)
          (goto-line (car (list-ref errs idx))))
        (message "No more errors"))))

(define (compilation-previous-error-no-select)
  (let* ((bufname (current-buffer))
         (errs (results-errors bufname))
         (idx (results-index-before errs (line-number))))
    (if idx
        (begin
          (set! *next-error-buffer* bufname)
          (goto-line (car (list-ref errs idx))))
        (message "Moved back before first error"))))

;; Global next-error/previous-error (M-g n / M-g p / C-x `): step through
;; the last-used results buffer from anywhere.
(define (next-error)
  (if (equal? *next-error-buffer* #f)
      (message "No compilation session")
      (results-jump *next-error-buffer*
                    (+ (results-local *next-error-buffer* "compilation-current" -1) 1))))

(define (previous-error)
  (if (equal? *next-error-buffer* #f)
      (message "No compilation session")
      (results-jump *next-error-buffer*
                    (- (results-local *next-error-buffer* "compilation-current" -1) 1))))

;; ---- the asynchronous compile pipeline ----------------------------------------

;; Process filter: reassemble complete lines across chunk boundaries and
;; parse each one exactly once. `start` is the char offset the chunk landed
;; at (the pump appended it before calling us), so face spans can be placed
;; without re-measuring the buffer.
(define (compilation-filter bufname text start)
  (let* ((partial (results-local bufname "compilation-partial" ""))
         (base (- start (string-length partial)))
         (parts (string-split-char (string-append partial text) "\n")))
    (let loop ((ps parts) (o base) (ln (+ (results-local bufname "compilation-line" 0) 1)))
      (if (null? (cdr ps))
          (begin
            (buffer-local-set-in! bufname "compilation-partial" (car ps))
            (buffer-local-set-in! bufname "compilation-partial-offset" o)
            (buffer-local-set-in! bufname "compilation-line" (- ln 1)))
          (begin
            (results-parse-line bufname (car ps) ln o)
            (loop (cdr ps) (+ o (string-length (car ps)) 1) (+ ln 1)))))))

;; Process sentinel: parse a trailing line that never got its newline, then
;; annotate the buffer and the mode line with the outcome, Emacs-style.
(define (compilation-sentinel bufname code)
  (let ((partial (results-local bufname "compilation-partial" "")))
    (unless (equal? partial "")
      (results-parse-line bufname partial
                          (+ (results-local bufname "compilation-line" 0) 1)
                          (results-local bufname "compilation-partial-offset" 0))
      (buffer-local-set-in! bufname "compilation-partial" "")))
  (let ((outcome (cond ((equal? code 0) "finished")
                       ((< code 0) "killed")
                       (else (string-append "exited abnormally with code "
                                            (number->string code))))))
    (buffer-append-in! bufname
                       (string-append "\nCompilation " outcome " at "
                                      (current-time-string) "\n"))
    (set-buffer-mode-name-in! bufname
                              (string-append "Compilation:exit ["
                                             (number->string code) "]"))
    (message (string-append "Compilation " outcome))))

(define (compilation-start cmd)
  (set! *compile-command* cmd)
  (let ((dir (default-directory))
        (from-results (equal? (current-buffer) "*compilation*"))
        (gen (+ (results-local "*compilation*" "compilation-generation" 0) 1)))
    ;; A compile still running in this buffer dies first; its generation
    ;; guard (below) keeps its late sentinel from scribbling on our run,
    ;; and process-kill makes Rust drop its in-flight output.
    (let ((old (buffer-local-get-in "*compilation*" "compilation-process")))
      (unless (equal? old #f)
        (when (process-live? old)
          (process-kill old))))
    (unless from-results
      (other-window-or-split))
    (switch-to-buffer "*compilation*")
    (buffer-clear-face-spans! "*compilation*")
    (let ((header (string-append
                   "-*- mode: compilation; default-directory: \"" dir "\" -*-\n"
                   "Compilation started at " (current-time-string) "\n\n"
                   cmd "\n")))
      (set-buffer-string! header)
      (set-buffer-read-only! #t)
      (set-buffer-mode-name "Compilation:run")
      (use-local-map "compilation-mode-map")
      (buffer-local-set! "compilation-errors" '())
      (buffer-local-set! "compilation-current" -1)
      (buffer-local-set! "compilation-dir" dir)
      (buffer-local-set! "compilation-command" cmd)
      (buffer-local-set! "compilation-generation" gen)
      (buffer-local-set! "compilation-partial" "")
      (buffer-local-set! "compilation-partial-offset" (string-length header))
      (buffer-local-set! "compilation-last-type" "error")
      (buffer-local-set! "compilation-line" (results-count-lines header))
      ;; Point at end of buffer => this window follows the streamed output
      ;; (Editor::append_to_buffer's tail-follow rule).
      (goto-char (string-length header))
      (set! *next-error-buffer* "*compilation*")
      (buffer-local-set! "compilation-process"
        (start-process "compilation" "*compilation*" dir cmd
          (lambda (text start end)
            (when (equal? gen (results-local "*compilation*" "compilation-generation" 0))
              (compilation-filter "*compilation*" text start)))
          (lambda (code)
            (when (equal? gen (results-local "*compilation*" "compilation-generation" 0))
              (compilation-sentinel "*compilation*" code))))))
    ;; Emacs leaves you in the window you compiled from.
    (unless from-results
      (other-window))))

;; ---- commands ----------------------------------------------------------------

(define (compile)
  (read-string "Compile command: " *compile-command* "" compilation-start))

(define (recompile)
  (let ((cmd (results-local "*compilation*" "compilation-command" #f)))
    (if cmd
        (compilation-start cmd)
        (message "No previous compilation"))))

(define (kill-compilation)
  (let ((id (buffer-local-get-in "*compilation*" "compilation-process")))
    (if (and (not (equal? id #f)) (process-live? id))
        (begin
          (process-kill id)
          (message "Killed compilation"))
        (message "No compilation running"))))

(define (quit-window) (delete-window))

(define-command "compile"
  "Run a compile command asynchronously; output and errors land in *compilation*."
  compile)
(define-command "recompile"
  "Re-run the last compilation."
  recompile)
(define-command "kill-compilation"
  "Kill the running compilation process."
  kill-compilation)
(define-command "compile-goto-error"
  "Visit the source of the error message at point."
  compile-goto-error)
(define-command "compilation-next-error-no-select"
  "Move to the next error message line."
  compilation-next-error-no-select)
(define-command "compilation-previous-error-no-select"
  "Move to the previous error message line."
  compilation-previous-error-no-select)
(define-command "next-error"
  "Visit the next error of the last compilation (or grep)."
  next-error)
(define-command "previous-error"
  "Visit the previous error of the last compilation (or grep)."
  previous-error)
(define-command "quit-window"
  "Delete the selected window."
  quit-window)

;; ---- keymaps -------------------------------------------------------------------

(define-key "compilation-mode-map" "RET" "compile-goto-error")
(define-key "compilation-mode-map" "n"   "compilation-next-error-no-select")
(define-key "compilation-mode-map" "p"   "compilation-previous-error-no-select")
(define-key "compilation-mode-map" "g"   "recompile")
(define-key "compilation-mode-map" "q"   "quit-window")
(define-key "compilation-mode-map" "C-c C-k" "kill-compilation")

(global-set-key "M-g n" "next-error")
(global-set-key "M-g p" "previous-error")
(global-set-key "C-x `" "next-error")
