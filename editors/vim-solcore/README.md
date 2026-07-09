# Solcore Vim/Neovim support

This directory provides Vim runtime files for Solcore `.solc` files:

- `ftdetect/solcore.vim` detects `*.solc` as the `solcore` filetype.
- `ftplugin/solcore.vim` configures comments, formatting, suffix lookup, and word
  movement for Solcore buffers.
- `syntax/solcore.vim` provides Vim script syntax highlighting.
- `lua/solcore/init.lua` provides a Neovim native LSP helper.

## Installation

Add this directory to Vim or Neovim's `runtimepath`. For example, with a local
checkout:

```vim
set runtimepath+=/path/to/solcore-rs/editors/vim-solcore
```

Plugin managers can point at this directory as a local plugin.

## Syntax Highlighting

Open any `.solc` file after the runtime path is configured. Vim/Neovim will set
`filetype=solcore` and load `syntax/solcore.vim` when syntax highlighting is
enabled:

```vim
syntax enable
```

## Neovim LSP

Build or install the native stdio language server first:

```sh
cargo install --path crates/lsp --features native
```

The helper uses `SOLCORE_LSP_SERVER` when it is set. Otherwise it starts
`solcore-lsp` from `PATH`.

```sh
export SOLCORE_LSP_SERVER=/path/to/solcore-lsp
```

Then enable the helper from `init.lua`:

```lua
require("solcore").setup()
```

You can override the command or add normal Neovim LSP options:

```lua
require("solcore").setup({
  cmd = { "/path/to/solcore-lsp" },
  on_attach = function(client, bufnr)
    -- Configure buffer-local LSP mappings here.
  end,
})
```

The LSP helper starts only for buffers with `filetype=solcore`.
