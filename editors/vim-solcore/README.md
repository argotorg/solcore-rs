# Solcore for Vim and Neovim

This package provides `.sol` file detection, syntax highlighting, indentation
settings, and helpers for starting the compiler-integrated language server.

A `.sol` buffer gets `filetype=solcore` only when it starts with
`pragma solcore;` (comments may precede it). Other `.sol` files keep the
built-in Solidity detection. A new file becomes `solcore` when it is written
with the pragma.

## Install

Download and extract the `vim-solcore` editor archive from GitHub Releases into
a native package directory, for example:

- Neovim: `~/.local/share/nvim/site/pack/solcore/start/vim-solcore`
- Vim: `~/.vim/pack/solcore/start/vim-solcore`

The resulting package directory must directly contain `ftdetect/`, `syntax/`,
`autoload/`, and `lua/`. A local checkout can instead be added to `runtimepath`:

```vim
set runtimepath+=/path/to/solcore-rs/editors/vim-solcore
syntax enable
```

Install the compiler release separately and keep its `std/` directory next to
`solcore`. Updating the compiler updates the server; restart the editor to use
the new server. Replace the editor package directory to update client support.

## Neovim

Enable the native LSP helper from `init.lua` (Neovim 0.8 or newer):

```lua
require("solcore").setup()
```

Automatic discovery uses `SOLCORE_PATH`, legacy `SOLCORE_LSP_SERVER`, then
`$CARGO_HOME/bin` (or `~/.cargo/bin`), `~/.local/bin`, and absolute directories
on `PATH`. Normal compiler discovery starts `solcore lsp`; the legacy variable
starts a standalone server without arguments.

Set an explicit compiler path or supply a complete command:

```lua
require("solcore").setup({
  path = "/path/to/solcore",
  on_attach = function(client, bufnr)
    -- Configure buffer-local LSP mappings here.
  end,
})
-- For an existing standalone installation:
-- require("solcore").setup({ cmd = { "/path/to/solcore-lsp" } })
```

`cmd`/`command` takes precedence over `path`, which takes precedence over
automatic discovery. The helper starts only for `filetype=solcore` buffers.

## Vim

Vim syntax support needs no LSP client. For language-server features, install a
client such as [vim-lsp](https://github.com/prabirshrestha/vim-lsp) and register:

```vim
augroup solcore_lsp
  autocmd!
  autocmd User lsp_setup call lsp#register_server({
        \ 'name': 'solcore',
        \ 'cmd': {server_info -> solcore#lsp_command()},
        \ 'allowlist': ['solcore'],
        \ })
augroup END
```

Set `g:solcore_path` for an explicit compiler path, or
`g:solcore_lsp_command` to a list containing the full
command and arguments. Otherwise the same environment and install-directory
search order as Neovim applies.
