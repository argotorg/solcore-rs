# Solcore Emacs mode

This directory contains Emacs support for Solcore `.solc` files:

- `solcore-mode.el` provides a `prog-mode`-derived major mode.
- `.solc` files are added to `auto-mode-alist`.
- Syntax highlighting uses Emacs font-lock for Solcore keywords, declarations,
  primitive types, constants, numbers, operators, and function calls.
- Optional LSP registration is provided for both `lsp-mode` and Eglot.

## Install

Add this directory to `load-path` and load the mode:

```elisp
(add-to-list 'load-path "/path/to/solcore-rs/editors/emacs-solcore")
(require 'solcore-mode)
```

With `use-package`:

```elisp
(use-package solcore-mode
  :load-path "/path/to/solcore-rs/editors/emacs-solcore"
  :mode ("\\.solc\\'" . solcore-mode))
```

## LSP server command

The mode resolves the native stdio LSP server command in this order:

1. Use `SOLCORE_LSP_SERVER` when the environment variable is set and non-empty.
2. Otherwise run `solcore-lsp` from `PATH`.

For example:

```sh
export SOLCORE_LSP_SERVER=/absolute/path/to/solcore-lsp
emacs
```

You can also set it from Emacs before starting the LSP client:

```elisp
(setenv "SOLCORE_LSP_SERVER" "/absolute/path/to/solcore-lsp")
```

If you want a fallback other than `solcore-lsp`, customize
`solcore-lsp-server-command`:

```elisp
(setq solcore-lsp-server-command '("/absolute/path/to/solcore-lsp"))
```

## lsp-mode

`solcore-mode` registers an `lsp-mode` client after `lsp-mode` is loaded.
Enable it with a hook:

```elisp
(use-package lsp-mode
  :commands (lsp lsp-deferred))

(use-package solcore-mode
  :load-path "/path/to/solcore-rs/editors/emacs-solcore"
  :mode ("\\.solc\\'" . solcore-mode)
  :hook (solcore-mode . lsp-deferred))
```

## Eglot

`solcore-mode` registers an Eglot server entry after Eglot is loaded. Enable it
with a hook:

```elisp
(use-package eglot
  :hook (solcore-mode . eglot-ensure))

(use-package solcore-mode
  :load-path "/path/to/solcore-rs/editors/emacs-solcore"
  :mode ("\\.solc\\'" . solcore-mode))
```

For non-`use-package` setups:

```elisp
(require 'solcore-mode)
(require 'eglot)
(add-hook 'solcore-mode-hook #'eglot-ensure)
```

## Manual checks

Open any `.solc` file and run:

```elisp
M-x solcore-mode
M-x lsp-deferred
```

or:

```elisp
M-x solcore-mode
M-x eglot
```
