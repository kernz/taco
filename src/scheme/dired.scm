;; dired, entirely in Steel: a directory listing rendered into a read-only
;; buffer with its own keymap. This is the reference case for the
;; Rust-core-as-VM boundary — everything here is built on narrow, policy-free
;; Rust primitives (directory-entries, rename-file, run-shell-command, ...);
;; Rust itself has no idea "dired" exists. It's loaded right after
;; bootstrap.scm (see main.rs / scheme::load_dired) rather than living in
;; examples/, since it's spec-mandated core behavior, not an opt-in plugin.
;;
;; Buffer-local state (see (buffer-local-set!/-get)):
;;   "dired-directory"      — absolute path string being listed
;;   "dired-show-hidden"    — #t/#f
;;   "dired-entries"        — list of dired-entry structs, sorted, in the
;;                            exact order rendered (line N+2 <-> entries[N])
;;   "dired-name-col"       — char column where file names start
;;   "dired-wgrep-snapshot" — #f, or a list of paths (see wgrep below)

;; ---- entry records --------------------------------------------------------

(struct dired-entry (path name is-dir mode-str nlink owner group size mtime mark)
  #:transparent)

(define (dired-entry-with-mark e mark)
  (dired-entry (dired-entry-path e) (dired-entry-name e) (dired-entry-is-dir e)
               (dired-entry-mode-str e) (dired-entry-nlink e) (dired-entry-owner e)
               (dired-entry-group e) (dired-entry-size e) (dired-entry-mtime e) mark))

(define (dired-raw->entry r mark)
  (dired-entry (list-ref r 0) (list-ref r 1) (list-ref r 2) (list-ref r 3)
               (list-ref r 4) (list-ref r 5) (list-ref r 6) (list-ref r 7)
               (list-ref r 8) mark))

(define (dired-find-by-name name entries)
  (cond ((null? entries) #f)
        ((equal? (dired-entry-name (car entries)) name) (car entries))
        (else (dired-find-by-name name (cdr entries)))))

;; Directories first, then case-insensitive name — ".." always sorts first
;; among directories since "." (46) sorts below any letter.
(define (dired-entry-less? a b)
  (cond ((and (dired-entry-is-dir a) (not (dired-entry-is-dir b))) #t)
        ((and (not (dired-entry-is-dir a)) (dired-entry-is-dir b)) #f)
        (else (string<? (string-downcase (dired-entry-name a))
                         (string-downcase (dired-entry-name b))))))

;; Re-read `dir` from disk, preserving marks by file name against
;; `old-entries` (a rename won't survive this, matching Emacs dired).
(define (dired-load-entries dir show-hidden old-entries)
  (sort (map (lambda (r)
               (let ((old (dired-find-by-name (list-ref r 1) old-entries)))
                 (dired-raw->entry r (if old (dired-entry-mark old) " "))))
             (directory-entries dir show-hidden))
        dired-entry-less?))

;; ---- listing text -----------------------------------------------------------

(define (spaces n)
  (if (<= n 0) "" (string-append " " (spaces (- n 1)))))

(define (pad-left s w) (string-append (spaces (- w (string-length s))) s))
(define (pad-right s w) (string-append s (spaces (- w (string-length s)))))

(define (dired-max-width f entries)
  (apply max 1 (map (lambda (e) (string-length (f e))) entries)))

(define (dired-format-line e lw ow gw sw)
  (string-append (dired-entry-mark e) " "
                  (dired-entry-mode-str e) " "
                  (pad-left (number->string (dired-entry-nlink e)) lw) " "
                  (pad-right (dired-entry-owner e) ow) " "
                  (pad-right (dired-entry-group e) gw) " "
                  (pad-left (number->string (dired-entry-size e)) sw) " "
                  (dired-entry-mtime e) " "
                  (dired-entry-name e)))

;; -> (text name-col): the buffer text (ls -la style) and the char column
;; where file names start on every entry line (metadata columns are
;; width-aligned per listing, so this depends on the entry set).
(define (dired-listing-text dir entries)
  (let* ((lw (dired-max-width (lambda (e) (number->string (dired-entry-nlink e))) entries))
         (ow (dired-max-width dired-entry-owner entries))
         (gw (dired-max-width dired-entry-group entries))
         (sw (dired-max-width (lambda (e) (number->string (dired-entry-size e))) entries))
         (name-col (+ 2 10 1 lw 1 ow 1 gw 1 sw 1 12 1))
         (header (string-append "  " dir ":"))
         (body (apply string-append header
                       (map (lambda (e) (string-append "\n" (dired-format-line e lw ow gw sw)))
                            entries))))
    (list body name-col)))

;; Reformat the buffer from the in-memory entry list — no disk access, used
;; after a mark change. Keeps point on the same line.
(define (dired-render!)
  (let* ((dir (buffer-local-get "dired-directory"))
         (entries (buffer-local-get "dired-entries"))
         (line (line-number))
         (lt (dired-listing-text dir entries)))
    (buffer-local-set! "dired-name-col" (list-ref lt 1))
    (set-buffer-string! (list-ref lt 0))
    (set-buffer-read-only! #t)
    (goto-line line)))

;; Re-read the directory from disk (preserving marks), then render.
(define (dired-refresh!)
  (let* ((dir (buffer-local-get "dired-directory"))
         (show-hidden (buffer-local-get "dired-show-hidden"))
         (old (buffer-local-get "dired-entries"))
         (old (if (list? old) old '()))
         (entries (dired-load-entries dir show-hidden old)))
    (buffer-local-set! "dired-entries" entries)
    (dired-render!)))

;; ---- open / entry points -----------------------------------------------------

(define (dired-buffer? name)
  (not (equal? (buffer-local-get-in name "dired-directory") #f)))

(define (dired-find-buffer-for dir names)
  (cond ((null? names) #f)
        ((equal? (buffer-local-get-in (car names) "dired-directory") dir) (car names))
        (else (dired-find-buffer-for dir (cdr names)))))

;; Open (or reuse) a dired buffer for `dir` in the selected window. This is
;; the one hook Rust calls into (via register-directory-opener) whenever
;; find-file lands on a directory.
(define (open-dired dir)
  (let* ((dir (canonicalize-path dir))
         (existing (dired-find-buffer-for dir (buffer-list))))
    (if existing
        (begin (switch-to-buffer existing) (dired-refresh!))
        (begin
          (switch-to-buffer dir)
          (set-buffer-mode-name "Dired")
          (use-local-map "dired-mode-map")
          (buffer-local-set! "dired-directory" dir)
          (buffer-local-set! "dired-show-hidden" #f)
          (buffer-local-set! "dired-entries" '())
          (buffer-local-set! "dired-wgrep-snapshot" #f)
          (dired-refresh!)))))

(register-directory-opener open-dired)

;; The Scheme-facing (dired path) entry point — a plain wrapper now that
;; open-dired lives here too.
(define (dired path) (open-dired (resolve-path path)))

(define (dired-ends-with-slash? s)
  (and (> (string-length s) 0)
       (equal? (substring s (- (string-length s) 1) (string-length s)) "/")))

(define (dired-open-dir-cmd)
  (let* ((dir (default-directory))
         (prefill (if (dired-ends-with-slash? dir) dir (string-append dir "/"))))
    (read-string "Dired (directory): " prefill "file"
                 (lambda (input) (open-dired (resolve-path input))))))

(define (dired-current) (open-dired (default-directory)))

(define (dired-project-root)
  (let loop ((dir (default-directory)))
    (if (file-exists? (string-append dir "/.git"))
        (open-dired dir)
        (let ((parent (parent-directory dir)))
          (if (equal? parent "")
              (message "No project root found (no .git in any ancestor)")
              (loop parent))))))

;; ---- navigation ---------------------------------------------------------------

;; Entry index (0-based into "dired-entries") for the line point is on, or
;; #f off the listing. Line 1 (1-based) is the header; entries start at 2.
(define (dired-entry-index)
  (let ((idx (- (line-number) 2)))
    (if (and (>= idx 0) (< idx (length (buffer-local-get "dired-entries"))))
        idx
        #f)))

(define (dired-entry-at-point)
  (let ((idx (dired-entry-index)))
    (if idx (list-ref (buffer-local-get "dired-entries") idx) #f)))

(define (dired-move-to-name-col)
  (beginning-of-line)
  (goto-char (+ (point) (buffer-local-get "dired-name-col"))))

(define (dired-goto-entry name)
  (let loop ((entries (buffer-local-get "dired-entries")) (i 0))
    (cond ((null? entries) #f)
          ((equal? (dired-entry-name (car entries)) name)
           (goto-line (+ i 2))
           (dired-move-to-name-col))
          (else (loop (cdr entries) (+ i 1))))))

(define (dired-find-file)
  (let ((e (dired-entry-at-point)))
    (if (not e)
        (message "No file on this line")
        (if (dired-entry-is-dir e)
            (open-dired (dired-entry-path e))
            (find-file (dired-entry-path e))))))

(define (dired-find-file-other-window)
  (let ((e (dired-entry-at-point)))
    (if (not e)
        (message "No file on this line")
        (begin
          (other-window-or-split)
          (if (dired-entry-is-dir e)
              (open-dired (dired-entry-path e))
              (find-file (dired-entry-path e)))))))

(define (dired-up-directory)
  (let ((parent (parent-directory (buffer-local-get "dired-directory"))))
    (if (equal? parent "")
        (message "At filesystem root")
        (open-dired parent))))

;; C-x C-j: dired on the current buffer's directory, point on that file.
(define (dired-jump)
  (let ((from (buffer-file-name)))
    (open-dired (default-directory))
    (if (equal? from "") #f (dired-goto-entry (file-name-only from)))))

;; ---- marking --------------------------------------------------------------

(define (dired-require!)
  (if (equal? (buffer-local-get "dired-directory") #f)
      (begin (message "Not a dired buffer") #f)
      #t))

;; Marking, deleting, renaming etc. never apply to the parent entry.
(define (dired-refuse-dotdot? e)
  (if (equal? (dired-entry-name e) "..")
      (begin (message "Cannot operate on `..'") #t)
      #f))

(define (dired-update-mark! name mark)
  (buffer-local-set! "dired-entries"
    (map (lambda (e) (if (equal? (dired-entry-name e) name) (dired-entry-with-mark e mark) e))
         (buffer-local-get "dired-entries"))))

(define (dired-set-mark-at-point! mark)
  (let ((e (dired-entry-at-point)))
    (cond ((not e) (message "No file on this line"))
          ((dired-refuse-dotdot? e) #f)
          (else
           (dired-update-mark! (dired-entry-name e) mark)
           (dired-render!)
           (next-line)))))

(define (dired-mark) (dired-set-mark-at-point! "*"))
(define (dired-unmark) (dired-set-mark-at-point! " "))
(define (dired-flag-deletion) (dired-set-mark-at-point! "D"))

(define (dired-unmark-all)
  (if (dired-require!)
      (begin
        (buffer-local-set! "dired-entries"
          (map (lambda (e) (dired-entry-with-mark e " ")) (buffer-local-get "dired-entries")))
        (dired-render!)
        (message "All marks removed"))))

(define (dired-mark-regexp-cmd)
  (if (dired-require!)
      (read-string "Mark files (regexp): " "" "" dired-mark-regexp)))

(define (dired-mark-regexp pattern)
  (let* ((entries (buffer-local-get "dired-entries"))
         (matches? (lambda (e) (and (not (equal? (dired-entry-name e) ".."))
                                     (regexp-match? pattern (dired-entry-name e)))))
         (marked (map (lambda (e) (if (matches? e) (dired-entry-with-mark e "*") e)) entries))
         (count (length (filter matches? entries))))
    (buffer-local-set! "dired-entries" marked)
    (dired-render!)
    (message (string-append (number->string count) " files marked"))))

;; ---- file IO ----------------------------------------------------------------

(define (dired-delete-entries! entries)
  (let* ((results (map (lambda (e)
                          (cons (dired-entry-name e)
                                (if (dired-entry-is-dir e)
                                    (delete-directory-recursive (dired-entry-path e))
                                    (delete-file (dired-entry-path e)))))
                        entries))
         (errors (filter (lambda (r) (not (equal? (cdr r) ""))) results)))
    (dired-refresh!)
    (if (null? errors)
        (message (string-append "Deleted " (number->string (length entries)) " file(s)"))
        (message (string-join (map (lambda (r) (string-append (car r) ": " (cdr r))) errors) "; ")))))

(define (dired-do-delete)
  (let ((e (dired-entry-at-point)))
    (cond ((not e) (message "No file on this line"))
          ((dired-refuse-dotdot? e) #f)
          (else (y-or-n-p (string-append "Delete " (dired-entry-name e) "? (y or n) ")
                           (lambda () (dired-delete-entries! (list e))))))))

(define (dired-do-flagged-delete)
  (if (dired-require!)
      (let ((flagged (filter (lambda (e) (equal? (dired-entry-mark e) "D"))
                              (buffer-local-get "dired-entries"))))
        (if (null? flagged)
            (message "(No deletions requested)")
            (let ((n (length flagged)))
              (y-or-n-p (string-append "Delete " (number->string n) " file"
                                        (if (= n 1) "" "s") "? (y or n) ")
                        (lambda () (dired-delete-entries! flagged))))))))

(define (dired-do-rename)
  (let ((e (dired-entry-at-point)))
    (cond ((not e) (message "No file on this line"))
          ((dired-refuse-dotdot? e) #f)
          (else (read-string (string-append "Rename " (dired-entry-name e) " to: ")
                              (dired-entry-name e) ""
                              (lambda (to) (dired-rename-to! (dired-entry-path e) to)))))))

(define (dired-rename-to! from to)
  (let* ((dest (resolve-path to))
         (err (rename-file from dest)))
    (dired-refresh!)
    (if (equal? err "")
        (message (string-append "Renamed to " dest))
        (message (string-append "Rename failed: " err)))))

(define (dired-do-copy)
  (let ((e (dired-entry-at-point)))
    (cond ((not e) (message "No file on this line"))
          ((dired-refuse-dotdot? e) #f)
          (else (read-string (string-append "Copy " (dired-entry-name e) " to: ") "" ""
                              (lambda (to) (dired-copy-to! (dired-entry-path e) (dired-entry-is-dir e) to)))))))

(define (dired-copy-to! from is-dir to)
  (let* ((dest (resolve-path to))
         (err (if is-dir (copy-directory-recursive from dest) (copy-file from dest))))
    (dired-refresh!)
    (if (equal? err "")
        (message (string-append "Copied to " dest))
        (message (string-append "Copy failed: " err)))))

(define (dired-create-directory)
  (read-string "Create directory: " "" "" dired-mkdir))

(define (dired-mkdir name)
  (let* ((dest (resolve-path name))
         (err (make-directory dest)))
    (dired-refresh!)
    (if (equal? err "")
        (message (string-append "Created " dest))
        (message (string-append "mkdir failed: " err)))))

(define (dired-diff)
  (let ((e (dired-entry-at-point)))
    (if (not e)
        (message "No file on this line")
        (read-string (string-append "Diff " (dired-entry-name e) " against: ") "" ""
                      (lambda (other) (dired-diff-against! (dired-entry-path e) other))))))

(define (dired-diff-against! from other)
  (let* ((other-path (resolve-path other))
         (diff (diff-unified other-path (read-file-to-string other-path)
                              from (read-file-to-string from)))
         (text (if (equal? diff "") "(no differences)" diff)))
    (other-window-or-split)
    (switch-to-buffer "*diff*")
    (set-buffer-string! text)
    (set-buffer-read-only! #t)))

(define (dired-compress)
  (let ((e (dired-entry-at-point)))
    (cond ((not e) (message "No file on this line"))
          ((dired-refuse-dotdot? e) #f)
          (else
           (let ((out (if (dired-entry-is-dir e)
                          (tar-gzip-directory (dired-entry-path e))
                          (gzip-file (dired-entry-path e)))))
             (dired-refresh!)
             (if (equal? out "")
                 (message "Compress failed")
                 (message (string-append "Compressed to " out))))))))

(define (dired-revert)
  (if (dired-require!)
      (begin (dired-refresh!) (message "Directory re-read"))))

(define (dired-toggle-hidden)
  (if (dired-require!)
      (let ((showing (not (buffer-local-get "dired-show-hidden"))))
        (buffer-local-set! "dired-show-hidden" showing)
        (dired-refresh!)
        (message (if showing "Showing hidden files" "Hiding hidden files")))))

(define (dired-kill-all)
  (let ((names (filter dired-buffer? (buffer-list))))
    (for-each (lambda (n) (switch-to-buffer n) (kill-buffer)) names)
    (message (string-append "Killed " (number->string (length names)) " dired buffer"
                             (if (= (length names) 1) "" "s")))))

;; ---- shell command ------------------------------------------------------------

(define (dired-shell-command-cmd)
  (if (dired-require!)
      (read-string "! on marked files (shell command): " "" "" dired-run-shell)))

(define (dired-shell-targets)
  (let ((marked (filter (lambda (e) (equal? (dired-entry-mark e) "*"))
                         (buffer-local-get "dired-entries"))))
    (if (null? marked)
        (let ((e (dired-entry-at-point))) (if e (list e) '()))
        marked)))

(define (dired-quote-arg s) (string-append "'" (string-replace s "'" "'\\''") "'"))

(define (dired-shell-output-text stdout stderr)
  (let ((text (if (equal? stderr "") stdout (string-append stdout "\n--- stderr ---\n" stderr))))
    (if (equal? text "") "(no output)" text)))

(define (dired-run-shell cmd)
  (let ((targets (dired-shell-targets)))
    (if (null? targets)
        (message "No files to operate on")
        (let* ((quoted (map (lambda (e) (dired-quote-arg (dired-entry-path e))) targets))
               (full (string-append cmd " " (string-join quoted " ")))
               (out (run-shell-command full))
               (text (dired-shell-output-text (list-ref out 0) (list-ref out 1))))
          (other-window-or-split)
          (switch-to-buffer "*Shell Command Output*")
          (set-buffer-string! text)
          (set-buffer-read-only! #t)))))

;; ---- A: search files for a regexp ---------------------------------------------
;; dired-do-find-regexp: search the marked files (or the file at point)
;; and put every matching line into a grep-style *Find Regexp* buffer
;; built on compile.scm's results machinery — so RET / n / p there, and
;; M-g n anywhere, jump straight to each match.

(define (dired-do-find-regexp)
  (if (dired-require!)
      (read-string "Search marked files (regexp): " "" "" dired-find-regexp-run)))

;; "path:lineno:text" gnu-format lines for every matching line of `path`.
;; Binary files come back as "" from read-file-to-string (invalid UTF-8),
;; which naturally yields no matches. The trailing element of the split is
;; the empty tail after the final newline, not a real line.
(define (dired-find-regexp-file pattern path)
  (let loop ((ls (string-split-char (read-file-to-string path) "\n")) (ln 1) (acc ""))
    (cond ((null? ls) acc)
          ((and (null? (cdr ls)) (equal? (car ls) "")) acc)
          ((regexp-match? pattern (car ls))
           (loop (cdr ls) (+ ln 1)
                 (string-append acc path ":" (number->string ln) ":" (car ls) "\n")))
          (else (loop (cdr ls) (+ ln 1) acc)))))

(define (dired-find-regexp-run pattern)
  (let ((files (filter (lambda (e) (and (not (dired-entry-is-dir e))
                                        (not (equal? (dired-entry-name e) ".."))))
                       (dired-shell-targets)))
        (dir (buffer-local-get "dired-directory")))
    (if (null? files)
        (message "No files to search")
        (let ((text (apply string-append
                           (map (lambda (e)
                                  (dired-find-regexp-file pattern (dired-entry-path e)))
                                files))))
          (other-window-or-split)
          (switch-to-buffer "*Find Regexp*")
          (set-buffer-string!
           (if (equal? text "")
               (string-append "No matches for " pattern "\n")
               text))
          (set-buffer-read-only! #t)
          (set-buffer-mode-name "Grep")
          (use-local-map "compilation-mode-map")
          (results-parse-current-buffer! dir)
          (let ((n (length (results-errors "*Find Regexp*"))))
            (message (string-append (number->string n)
                                    (if (equal? n 1) " match" " matches"))))))))

;; ---- n/p: line motion that lands on the file name ------------------------------

(define (dired-next-line)
  (next-line)
  (when (dired-entry-index) (dired-move-to-name-col)))

(define (dired-previous-line)
  (previous-line)
  (when (dired-entry-index) (dired-move-to-name-col)))

;; ---- wgrep: bulk rename by editing the listing as plain text ------------------
;;
;; The snapshot maps entries positionally to buffer lines (offset by the
;; header): editing a name in place renames it. Reordering or deleting
;; lines is not tracked, exactly like the Rust version this replaces.

(define (wgrep-mode)
  (if (dired-require!)
      (if (not (equal? (buffer-local-get "dired-wgrep-snapshot") #f))
          (message "Already editing")
          (begin
            (buffer-local-set! "dired-wgrep-snapshot"
              (map (lambda (e) (if (equal? (dired-entry-name e) "..") #f (dired-entry-path e)))
                   (buffer-local-get "dired-entries")))
            (set-buffer-read-only! #f)
            (set-buffer-mode-name "Dired:Wgrep")
            (use-local-map "wgrep-mode-map")
            (message "Editable dired: edit names, then C-c C-c to commit or C-c C-k to abort")))))

(define (dired-zip a b)
  (cond ((or (null? a) (null? b)) '())
        (else (cons (list (car a) (car b)) (dired-zip (cdr a) (cdr b))))))

;; -> #f (untouched line) or (cons old-name error-string).
(define (dired-wgrep-rename-one old-path line name-col)
  (if (<= (string-length line) name-col)
      #f
      (let* ((new-name (substring line name-col (string-length line)))
             (old-name (file-name-only old-path)))
        (if (or (equal? new-name "") (equal? new-name old-name))
            #f
            (cons old-name
                  (rename-file old-path (string-append (parent-directory old-path) "/" new-name)))))))

(define (wgrep-leave!)
  (set-buffer-read-only! #t)
  (set-buffer-mode-name "Dired")
  (use-local-map "dired-mode-map"))

(define (wgrep-commit)
  (let ((snapshot (buffer-local-get "dired-wgrep-snapshot")))
    (if (equal? snapshot #f)
        (message "Not in wgrep mode (C-c C-e first)")
        (let* ((name-col (buffer-local-get "dired-name-col"))
               (pairs (dired-zip snapshot (list-tail (buffer-lines) 1)))
               (named (filter (lambda (p) (not (equal? (car p) #f))) pairs))
               (attempts (filter (lambda (x) x)
                                  (map (lambda (p) (dired-wgrep-rename-one (car p) (list-ref p 1) name-col))
                                       named)))
               (ok (filter (lambda (r) (equal? (cdr r) "")) attempts))
               (errs (filter (lambda (r) (not (equal? (cdr r) ""))) attempts)))
          (buffer-local-set! "dired-wgrep-snapshot" #f)
          (wgrep-leave!)
          (dired-refresh!)
          (if (null? errs)
              (message (string-append "Applied " (number->string (length ok)) " rename"
                                       (if (= (length ok) 1) "" "s")))
              (message (string-append (number->string (length ok)) " renamed; failed: "
                                       (string-join (map cdr errs) "; "))))))))

(define (wgrep-abort)
  (if (equal? (buffer-local-get "dired-wgrep-snapshot") #f)
      (message "Not in wgrep mode")
      (begin
        (buffer-local-set! "dired-wgrep-snapshot" #f)
        (wgrep-leave!)
        (dired-refresh!)
        (message "Changes aborted"))))

;; ---- commands & keymaps -------------------------------------------------------

(define-command "dired-open-dir" "Prompt for a directory and open it in dired." dired-open-dir-cmd)
(define-command "dired-current" "Open the current buffer's directory in dired." dired-current)
(define-command "dired-jump" "Open dired at the current file's directory, cursor on that file." dired-jump)
(define-command "dired-project-root" "Open the project root (nearest ancestor with .git) in dired." dired-project-root)
(define-command "dired-find-file" "Visit the file or directory at point." dired-find-file)
(define-command "dired-find-file-other-window" "Visit the file or directory at point in another window." dired-find-file-other-window)
(define-command "dired-up-directory" "Open the parent directory." dired-up-directory)
(define-command "dired-mark" "Mark the file at point." dired-mark)
(define-command "dired-mark-regexp" "Prompt for a regexp and mark all matching files." dired-mark-regexp-cmd)
(define-command "dired-shell-command" "Run a shell command on the marked files (or the file at point)." dired-shell-command-cmd)
(define-command "dired-flag-deletion" "Flag the file at point for deletion." dired-flag-deletion)
(define-command "dired-do-flagged-delete" "Delete the files flagged with D." dired-do-flagged-delete)
(define-command "dired-unmark" "Unmark the file at point." dired-unmark)
(define-command "dired-unmark-all" "Unmark all files." dired-unmark-all)
(define-command "dired-do-delete" "Delete the file at point." dired-do-delete)
(define-command "dired-do-rename" "Rename the file at point." dired-do-rename)
(define-command "dired-do-copy" "Copy the file at point." dired-do-copy)
(define-command "dired-create-directory" "Prompt for a name and create a directory." dired-create-directory)
(define-command "dired-diff" "Diff the file at point against another file." dired-diff)
(define-command "dired-compress" "Compress the file (gz) or directory (tar.gz) at point." dired-compress)
(define-command "dired-revert" "Refresh the dired listing." dired-revert)
(define-command "dired-toggle-hidden" "Toggle showing hidden files." dired-toggle-hidden)
(define-command "dired-kill-all" "Kill all dired buffers." dired-kill-all)
(define-command "dired-do-find-regexp" "Search the marked files (or file at point) for a regexp; jumpable results." dired-do-find-regexp)
(define-command "dired-next-line" "Move down a line, landing on the file name." dired-next-line)
(define-command "dired-previous-line" "Move up a line, landing on the file name." dired-previous-line)
(define-command "wgrep-mode" "Make the dired buffer writable to edit file names as plain text." wgrep-mode)
(define-command "wgrep-commit" "Apply the edited file names (renames files on disk)." wgrep-commit)
(define-command "wgrep-abort" "Abort wgrep editing and restore the listing." wgrep-abort)

;; ---- Dired entry points (global) -----------------------------------------------
(global-set-key "C-x C-j" "dired-jump")
(global-set-key "C-c f d" "dired-open-dir")
(global-set-key "C-c o -" "dired-current")
(global-set-key "C-c p D" "dired-project-root")

;; ---- Dired mode map -----------------------------------------------------------
(define-key "dired-mode-map" "RET" "dired-find-file")
(define-key "dired-mode-map" "o"   "dired-find-file-other-window")
(define-key "dired-mode-map" "^"   "dired-up-directory")
(define-key "dired-mode-map" "m"   "dired-mark")
(define-key "dired-mode-map" "% m" "dired-mark-regexp")
(define-key "dired-mode-map" "!"   "dired-shell-command")
(define-key "dired-mode-map" "d"   "dired-flag-deletion")
(define-key "dired-mode-map" "x"   "dired-do-flagged-delete")
(define-key "dired-mode-map" "u"   "dired-unmark")
(define-key "dired-mode-map" "U"   "dired-unmark-all")
(define-key "dired-mode-map" "D"   "dired-do-delete")
(define-key "dired-mode-map" "R"   "dired-do-rename")
(define-key "dired-mode-map" "C"   "dired-do-copy")
(define-key "dired-mode-map" "+"   "dired-create-directory")
(define-key "dired-mode-map" "="   "dired-diff")
(define-key "dired-mode-map" "Z"   "dired-compress")
(define-key "dired-mode-map" "g"   "dired-revert")
(define-key "dired-mode-map" ")"   "dired-toggle-hidden")
(define-key "dired-mode-map" "q"   "dired-kill-all")
(define-key "dired-mode-map" "A"   "dired-do-find-regexp")
(define-key "dired-mode-map" "n"   "dired-next-line")
(define-key "dired-mode-map" "p"   "dired-previous-line")
;; Emacs enters wdired with C-x C-q (dired-toggle-read-only); the spec for
;; taco names C-c C-q. Commit stays C-c C-c, abort C-c C-k (wgrep map).
(define-key "dired-mode-map" "C-c C-q" "wgrep-mode")

;; ---- Wgrep (writable dired) map --------------------------------------------------
(define-key "wgrep-mode-map" "C-c C-c" "wgrep-commit")
(define-key "wgrep-mode-map" "C-c C-k" "wgrep-abort")
