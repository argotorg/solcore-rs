# Solcore Emacs mode

This directory contains Emacs support for Solcore `.sol` files:

- `solcore-mode.el` provides a `prog-mode`-derived major mode.
- `.sol` files that start with `pragma solcore;` (after comments) open in
  `solcore-mode` through `magic-mode-alist`. Other `.sol` files are Solidity
  and are left to other modes, such as a Solidity mode.
- Syntax highlighting uses Emacs font-lock for Solcore keywords, declarations,
  primitive types, constants, numbers, operators, and function calls.
- Optional LSP registration is provided for both `lsp-mode` and Eglot.

## Install

Extract the `emacs-solcore` archive from GitHub Releases and add its directory
to `load-path`, then load the mode. Install the compiler release separately,
keeping `std/` next to `solcore`. Updating the compiler updates its server;
restart Eglot or lsp-mode to use the new version. Replace the mode directory to
update editor support.

```elisp
(add-to-list 'load-path "/path/to/solcore-rs/editors/emacs-solcore")
(require 'solcore-mode)
```

With `use-package`:

```elisp
(use-package solcore-mode
  :load-path "/path/to/solcore-rs/editors/emacs-solcore")
```

Do not map `\\.sol\\'` to `solcore-mode` in `auto-mode-alist`; that would also
open Solidity files as Solcore. After adding the pragma to a new file, run
`M-x normal-mode` to switch the buffer to `solcore-mode`.

## LSP server command

The default starts the compiler with `solcore lsp`. Discovery order is:

1. Non-nil `solcore-lsp-server-command` (complete command and arguments).
2. `SOLCORE_PATH` (compiler executable, automatically appending `lsp`).
3. Legacy `SOLCORE_LSP_SERVER` (standalone executable with no arguments).
4. `$CARGO_HOME/bin/solcore`, or `~/.cargo/bin/solcore` when unset.
5. `~/.local/bin/solcore`.
6. Absolute directories in Emacs `exec-path`.

Explicit configuration takes precedence over environment settings. For example:

```elisp
(setq solcore-lsp-server-command '("/absolute/path/to/solcore" "lsp"))
;; Existing standalone server installations remain supported:
;; (setq solcore-lsp-server-command '("/absolute/path/to/solcore-lsp"))
```

Or set `SOLCORE_PATH=/absolute/path/to/solcore` before launching Emacs. This
variable contains an executable path, not a shell command line. Reset
`solcore-lsp-server-command` to nil to restore automatic discovery.

## lsp-mode

`solcore-mode` registers an `lsp-mode` client after `lsp-mode` is loaded.
Enable it with a hook:

```elisp
(use-package lsp-mode
  :commands (lsp lsp-deferred))

(use-package solcore-mode
  :load-path "/path/to/solcore-rs/editors/emacs-solcore"
  :mode ("\\.sol\\'" . solcore-mode)
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
  :mode ("\\.sol\\'" . solcore-mode))
```

For non-`use-package` setups:

```elisp
(require 'solcore-mode)
(require 'eglot)
(add-hook 'solcore-mode-hook #'eglot-ensure)
```

## Manual checks

Open any `.sol` file and run:

```elisp
M-x solcore-mode
M-x lsp-deferred
```

or:

```elisp
M-x solcore-mode
M-x eglot
```
