;;; solcore-mode.el --- Major mode for Solcore -*- lexical-binding: t; -*-

;; Copyright (C) 2026 Solcore contributors

;; Author: Solcore contributors
;; Version: 0.1.0
;; Package-Requires: ((emacs "26.1"))
;; Keywords: languages
;; URL: https://github.com/Y-Nak/solcore-rs

;;; Commentary:

;; Major mode, font-lock highlighting, and optional LSP client registration for
;; Solcore `.solc' files.
;;
;; The LSP server command is resolved from SOLCORE_LSP_SERVER when that
;; environment variable is non-empty.  Otherwise `solcore-lsp-server-command'
;; is used, which defaults to running `solcore-lsp' from PATH.

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

(defcustom solcore-lsp-server-command '("solcore-lsp")
  "Fallback command used to start the Solcore LSP server.

SOLCORE_LSP_SERVER takes precedence when it is set to a non-empty
value.  This option may be a string command line or a list of the
program name followed by arguments."
  :type '(choice
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
  '("if" "else" "for" "while" "switch" "case" "default" "match"
    "return" "revert" "leave" "continue" "break" "unchecked"))

(defconst solcore--declaration-keywords
  '("contract" "interface" "library" "import" "from" "export" "as" "let"
    "alias" "enum" "struct" "trait" "impl" "where" "type" "is" "function"
    "returns" "constructor" "fallback" "assembly" "pragma" "lam"))

(defconst solcore--modifier-keywords
  '("public" "external" "internal" "private" "payable" "pure" "view"
    "comptime" "memory" "storage" "calldata"))

(defconst solcore--primitive-types
  '("address" "bool" "byte" "bytes" "bytes32" "int" "int256" "mapping" "string"
    "uint" "uint256" "unit" "word"))

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
  `((,(concat (solcore--keyword-prefix-regexp '("contract" "interface" "library"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-type-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("function"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-function-name-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("alias" "enum" "struct" "trait" "type"))
              "\\s-+\\(" solcore--identifier-re "\\)")
     (1 font-lock-keyword-face)
     (2 font-lock-type-face nil t))
    (,(concat (solcore--keyword-prefix-regexp '("let"))
              "\\s-+\\(?:comptime\\s-+\\)?\\(" solcore--identifier-re "\\)")
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
              (regexp-opt '(":=" "+=" "-=" "^=" "&=" "|=" "%=" "->" "=>"
                            "==" "!=" ">=" "<=" "&&" "||"))
              "\\|[+*/%!?=<>|&^@-]\\)")
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
    ("Contracts" ,(concat "^\\s-*\\(?:contract\\|interface\\|library\\)\\s-+\\("
                           solcore--identifier-re "\\)") 1)
    ("Types" ,(concat "^\\s-*\\(?:alias\\|enum\\|struct\\|trait\\|type\\)\\s-+\\("
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
  (let ((env-command (getenv "SOLCORE_LSP_SERVER")))
    (solcore--split-command
     (if (solcore--blank-string-p env-command)
         solcore-lsp-server-command
       env-command))))

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
  "Major mode for editing Solcore `.solc' files."
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
(add-to-list 'auto-mode-alist '("\\.solc\\'" . solcore-mode))

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
