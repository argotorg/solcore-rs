;;; solcore-mode.el --- Major mode for Solcore -*- lexical-binding: t; -*-

;; Copyright (C) 2026 Solcore contributors

;; Author: Solcore contributors
;; Version: 0.1.0-alpha
;; Package-Requires: ((emacs "26.1"))
;; Keywords: languages
;; URL: https://github.com/argotorg/solcore-rs

;;; Commentary:

;; Major mode, font-lock highlighting, and optional LSP client registration for
;; Solcore `.sol' files.
;;
;; The native language server is bundled in the compiler (`solcore lsp').
;; Explicit configuration overrides environment and installed compiler paths.

;;; Code:

(require 'rx)

(defgroup solcore nil
  "Editing support for the Solcore language."
  :group 'languages
  :prefix "solcore-")

(defcustom solcore-indent-offset 4
  "Number of spaces to indent nested Solcore blocks."
  :type 'integer
  :safe #'integerp
  :group 'solcore)

(defcustom solcore-lsp-server-command nil
  "Explicit command used to start the Solcore LSP server.

When nil, discover the compiler and run it with the lsp argument.
Otherwise this takes precedence over environment variables and may
be a command line or a list of the program followed by arguments."
  :type '(choice
          (const :tag "Discover installed Solcore" nil)
          (repeat :tag "Command and arguments" string)
          (string :tag "Shell-like command line"))
  :group 'solcore)

(defconst solcore--identifier-re
  "[[:alpha:]][[:alnum:]_]*\\(?:-[[:alpha:]][[:alnum:]_]*\\)*"
  "Regular expression matching a Solcore identifier.")

(defconst solcore--capitalized-identifier-re
  "[[:upper:]][[:alnum:]_]*\\(?:-[[:alpha:]][[:alnum:]_]*\\)*"
  "Regular expression matching a Solcore type-like identifier.")

(defconst solcore--symbol-edge-chars "[:alpha:][:digit:]_-"
  "Characters that keep a Solcore identifier or keyword going.")

(defconst solcore--control-keywords
  '("if" "else" "for" "while" "switch" "case" "default" "match" "return"
    "leave" "continue" "break"))

(defconst solcore--declaration-keywords
  '("contract" "import" "from" "hiding" "export" "as" "let" "enum" "trait"
    "impl" "where" "type" "function" "returns" "constructor" "fallback"
    "assembly" "pragma" "lam" "comptime" "derive"))

(defconst solcore--modifier-keywords
  '("public" "payable"))

(defconst solcore--primitive-types
  '("word" "bool" "string" "integer" "pair" "sum" "uint256" "address"
    "byte" "bytes" "bytes4" "bytes32" "memory" "storage" "calldata"
    "returndata" "mapping" "array"))

(defconst solcore--constants
  '("true" "false" "_"))

(defun solcore--keyword-regexp (keywords)
  "Return a regexp matching KEYWORDS with Solcore identifier boundaries.
The keyword itself is captured in group 1."
  (concat "\\(?:\\`\\|[^" solcore--symbol-edge-chars "]\\)"
          "\\(" (regexp-opt keywords) "\\)"
          "\\(?:\\'\\|[^" solcore--symbol-edge-chars "]\\)"))

(defun solcore--keyword-prefix-regexp (keywords)
  "Return a regexp matching KEYWORDS at a Solcore identifier boundary.
The keyword itself is captured in group 1.  The following character is
left to the caller so declaration patterns can consume whitespace once."
  (concat "\\(?:\\`\\|[^" solcore--symbol-edge-chars "]\\)"
          "\\(" (regexp-opt keywords) "\\)"))

(defconst solcore-font-lock-keywords
  `((,(concat (solcore--keyword-prefix-regexp '("contract"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-type-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("function"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-function-name-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("enum" "trait" "type"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-type-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("let"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-variable-name-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("pragma"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-preprocessor-face)
     (2 font-lock-preprocessor-face nil t))
    (,(solcore--keyword-regexp solcore--control-keywords)
     1 font-lock-keyword-face)
    (,(solcore--keyword-regexp solcore--declaration-keywords)
     1 font-lock-keyword-face)
    (,(solcore--keyword-regexp solcore--modifier-keywords)
     1 font-lock-builtin-face)
    (,(solcore--keyword-regexp solcore--primitive-types)
     1 font-lock-type-face)
    (,(solcore--keyword-regexp solcore--constants)
     1 font-lock-constant-face)
    (,(concat "\\(?:\\`\\|[^" solcore--symbol-edge-chars "]\\)"
              "\\(" solcore--capitalized-identifier-re "\\)")
     1 font-lock-type-face)
    (,(concat "\\(?:\\`\\|[^" solcore--symbol-edge-chars "]\\)"
              "\\(" solcore--identifier-re "\\)\\s-*(")
     1 font-lock-function-name-face)
    (,(concat "\\(?:\\`\\|[^[:alpha:][:digit:]_]\\)"
              "\\(0x[[:xdigit:]]+\\|[[:digit:]]+\\)"
              "\\(?:\\'\\|[^[:alpha:][:digit:]_]\\)")
     1 font-lock-constant-face)
    (,(concat "\\("
              (regexp-opt '(":=" "+=" "-=" "*=" "/=" "^=" "&=" "|=" "%=" "~=" "->" "=>"
                            "==" "!=" ">=" "<=" "&&" "||"))
              "\\|[+*/%!?=<>|&^@~-]\\)")
     1 font-lock-builtin-face))
  "Font-lock rules for `solcore-mode'.")

(defvar solcore-mode-syntax-table
  (let ((table (make-syntax-table)))
    (modify-syntax-entry ?_ "w" table)
    (modify-syntax-entry ?/ ". 124bn" table)
    (modify-syntax-entry ?* ". 23n" table)
    (modify-syntax-entry ?\n "> b" table)
    (modify-syntax-entry ?\" "\"" table)
    (modify-syntax-entry ?\\ "\\" table)
    table)
  "Syntax table for `solcore-mode'.")

(defvar solcore-imenu-generic-expression
  `((nil ,(concat "^\\s-*function\\s-+\\(" solcore--identifier-re "\\)") 1)
    ("Contracts" ,(concat "^\\s-*contract\\s-+\\(" solcore--identifier-re "\\)") 1)
    ("Types" ,(concat "^\\s-*\\(?:enum\\|trait\\|type\\)\\s-+\\("
                       solcore--identifier-re "\\)") 1))
  "Imenu expressions for `solcore-mode'.")

(defun solcore--blank-string-p (value)
  "Return non-nil when VALUE is nil or contains only whitespace."
  (or (null value)
      (string-match-p "\\`[[:space:]]*\\'" value)))

(defun solcore--split-command (command)
  "Normalize COMMAND to a list suitable for stdio LSP clients."
  (cond
   ((and (listp command) command) command)
   ((stringp command)
    (split-string-and-unquote command))
   (t
    (error "Invalid Solcore LSP server command: %S" command))))

(defun solcore--lsp-server-command ()
  "Return the command used to start the Solcore LSP server."
  (let* ((compiler (getenv "SOLCORE_PATH"))
         (legacy (getenv "SOLCORE_LSP_SERVER"))
         (cargo (getenv "CARGO_HOME"))
         (directories
          (append
           (list (if (solcore--blank-string-p cargo)
                     (expand-file-name "~/.cargo/bin")
                   (concat cargo "/bin"))
                 (expand-file-name "~/.local/bin"))
           exec-path))
         (name (if (eq system-type 'windows-nt) "solcore.exe" "solcore")))
    (cond
     (solcore-lsp-server-command
      (solcore--split-command solcore-lsp-server-command))
     ((not (solcore--blank-string-p compiler)) (list compiler "lsp"))
     ((not (solcore--blank-string-p legacy)) (list legacy))
     (t
      (let ((found
             (catch 'found
               (dolist (directory directories)
                 ;; nil/relative entries mean the current project directory.
                 (when (and directory (file-name-absolute-p directory))
                   (let ((candidate (expand-file-name name directory)))
                     (when (and (file-regular-p candidate)
                                (file-executable-p candidate))
                       (throw 'found candidate))))))))
        (unless found
          (user-error "Install Solcore or set solcore-lsp-server-command to '(\"/path/to/solcore\" \"lsp\")"))
        (list found "lsp"))))))

(defun solcore-calculate-indentation ()
  "Return the preferred indentation for the current line."
  (save-excursion
    (back-to-indentation)
    (let ((depth (car (syntax-ppss))))
      (when (looking-at-p "[]})]")
        (setq depth (max 0 (1- depth))))
      (* depth solcore-indent-offset))))

(defun solcore-indent-line ()
  "Indent the current line as Solcore source."
  (interactive)
  (let ((point-offset (- (point-max) (point))))
    (indent-line-to (solcore-calculate-indentation))
    (when (> (- (point-max) point-offset) (point))
      (goto-char (- (point-max) point-offset)))))

;;;###autoload
(define-derived-mode solcore-mode prog-mode "Solcore"
  "Major mode for editing Solcore `.sol' files."
  :syntax-table solcore-mode-syntax-table
  (setq-local font-lock-defaults '(solcore-font-lock-keywords))
  (setq-local comment-start "// ")
  (setq-local comment-end "")
  (setq-local comment-start-skip "\\(?://+\\|/\\*+\\)\\s-*")
  (setq-local comment-end-skip "\\s-*\\*/")
  (setq-local comment-use-syntax t)
  (setq-local indent-line-function #'solcore-indent-line)
  (setq-local imenu-generic-expression solcore-imenu-generic-expression)
  (setq-local electric-indent-chars
              (append "{}();," electric-indent-chars)))

;;;###autoload
(defun solcore-declares-pragma-p ()
  "Return non-nil when the buffer starts with `pragma solcore;'.
Leading whitespace and comments are skipped.  The compiler requires this
declaration first in every source file; it tells Solcore `.sol' files apart
from Solidity files, which use the same extension."
  (save-excursion
    (save-restriction
      (widen)
      (goto-char (point-min))
      (with-syntax-table solcore-mode-syntax-table
        (forward-comment (buffer-size)))
      (looking-at-p "pragma[ \t\n\r\f]+solcore[ \t\n\r\f]*;"))))

;;;###autoload
(defun solcore-source-buffer-p ()
  "Return non-nil for a `.sol' file buffer that declares `pragma solcore;'."
  (and buffer-file-name
       (string-match-p "\\.sol\\'" buffer-file-name)
       (solcore-declares-pragma-p)))

;; `.sol' files without the pragma are left to other modes, such as a
;; Solidity mode.
;;;###autoload
(add-to-list 'magic-mode-alist '(solcore-source-buffer-p . solcore-mode))

(defvar lsp-language-id-configuration)
(declare-function lsp-activate-on "lsp-mode")
(declare-function lsp-register-client "lsp-mode")
(declare-function lsp-stdio-connection "lsp-mode")
(declare-function make-lsp-client "lsp-mode")

(defun solcore-lsp-register-client ()
  "Register the Solcore language server with lsp-mode."
  (interactive)
  (require 'lsp-mode)
  (add-to-list 'lsp-language-id-configuration '(solcore-mode . "solcore"))
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection #'solcore--lsp-server-command)
    :activation-fn (lsp-activate-on "solcore")
    :major-modes '(solcore-mode)
    :server-id 'solcore-lsp)))

(with-eval-after-load 'lsp-mode
  (solcore-lsp-register-client))

(defvar eglot-server-programs)

(defun solcore--eglot-server-program (&rest _ignored)
  "Return the Solcore server command in Eglot contact form."
  (solcore--lsp-server-command))

(defun solcore-eglot-register ()
  "Register the Solcore language server with Eglot."
  (interactive)
  (require 'eglot)
  (setq eglot-server-programs
        (assq-delete-all 'solcore-mode eglot-server-programs))
  (add-to-list 'eglot-server-programs
               '(solcore-mode . solcore--eglot-server-program)))

(with-eval-after-load 'eglot
  (solcore-eglot-register))

(provide 'solcore-mode)

;;; solcore-mode.el ends here
