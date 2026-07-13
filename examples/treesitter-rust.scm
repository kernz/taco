;; treesitter-rust.scm — real tree-sitter syntax highlighting for Rust
;; files, as a user plugin.
;;
;; NOTE: taco now ships a built-in rust-mode (src/scheme/rust-mode.scm) that
;; already does exactly this — auto-installs this same grammar and sets
;; these same face colors the first time a .rs file is opened, no init.scm
;; needed. This file is kept as a template for wiring up tree-sitter for
;; some *other* language (swap the URL/extension pair below); loading it
;; alongside rust-mode for "rs" itself is harmless but redundant.
;;
;; Install: copy this into ~/.config/taco/init.scm (or append it to your
;; existing config). Pure Scheme over the public contract:
;;   * (tree-sit-install-language-grammar name url) clones the grammar's git
;;     repo, compiles it with the system C compiler, and loads it at
;;     runtime — the same thing Emacs' treesit-install-language-grammar
;;     does. It's a one-time cost: the compiled grammar is cached (in the OS
;;     cache dir) and the clone is skipped if it's already there, so this is
;;     safe to leave in your init.scm and run on every startup.
;;   * (tree-sit-enable-for-extension ext lang), defined in bootstrap.scm,
;;     registers which installed language a file extension should use;
;;     `find-file` then enables it automatically via "find-file-hook".
;;   * (set-face-color name color) colors a tree-sitter capture name
;;     ("keyword", "string", "comment", ...) exactly like it already colors
;;     "mode-line"/"highlight"/"line-number" — no separate API.
;;
;; Swap the URL/extension pair to add another language (e.g.
;; https://github.com/tree-sitter/tree-sitter-python for "py").

(tree-sit-install-language-grammar "rust" "https://github.com/tree-sitter/tree-sitter-rust")
(tree-sit-enable-for-extension "rs" "rust")

(set-face-color "keyword" "magenta")
(set-face-color "string" "green")
(set-face-color "comment" "cyan")
(set-face-color "type" "yellow")
(set-face-color "type.builtin" "yellow")
(set-face-color "function" "blue")
(set-face-color "function.method" "blue")
(set-face-color "constant" "red")
(set-face-color "constant.builtin" "red")
