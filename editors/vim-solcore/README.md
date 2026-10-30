# Solcore for Vim and Neovim

This package provides `.sol` file detection, syntax highlighting, indentation
settings, and helpers for starting the compiler-integrated language server.

A `.sol` buffer gets `filetype=solcore` only when it starts with
`pragma solcore;` (comments may precede it). Other `.sol` files keep the
built-in Solidity detection. A new file becomes `solcore` when it is written
with the pragma.

## Install

Download the editor archive from the same release as the
[compiler](https://github.com/argotorg/solcore-rs#installation), verify it, and
extract it into a native package directory. For Neovim:

```sh
version=0.1.0-alpha
archive="solcore-vim-v${version}.tar.gz"
url="https://github.com/argotorg/solcore-rs/releases/download/v${version}"
pack="$HOME/.local/share/nvim/site/pack/solcore/start"

cd "$(mktemp -d)"
curl -fLO "$url/SHA256SUMS"
curl -fLO "$url/$archive"
awk -v file="$archive" '$2 == file' SHA256SUMS | shasum -a 256 --check
mkdir -p "$pack"
tar -xzf "$archive" -C "$pack"
```

For Vim, use `pack="$HOME/.vim/pack/solcore/start"`. On Linux, use
`sha256sum --check`. The archive creates `vim-solcore/` in that directory; it
must directly contain `ftdetect/`, `syntax/`, `autoload/`, and `lua/`. A local
checkout can instead be added to `runtimepath`:

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
