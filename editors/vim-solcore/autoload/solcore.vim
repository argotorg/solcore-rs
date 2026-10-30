" Whether the current buffer starts with `pragma solcore;` after leading
" whitespace and comments. The compiler requires this declaration first in
" every source file; it tells Solcore `.sol` files apart from Solidity files.
function! solcore#declares_pragma() abort
  let in_comment = 0
  for line in getline(1, min([line('$'), 200]))
    let rest = line
    while 1
      if in_comment
        let end = stridx(rest, '*/')
        if end < 0
          break
        endif
        let rest = rest[end + 2 :]
        let in_comment = 0
      endif
      let rest = substitute(rest, '^[ \t\r\f]*', '', '')
      if rest ==# '' || rest =~# '^//'
        break
      elseif rest =~# '^/\*'
        let rest = rest[2 :]
        let in_comment = 1
      else
        return rest =~# '^pragma[ \t]\+solcore[ \t]*;'
      endif
    endwhile
  endfor
  return 0
endfunction

" Command factory for Vim LSP clients such as vim-lsp.
function! solcore#lsp_command() abort
  if exists('g:solcore_lsp_command') && !empty(g:solcore_lsp_command)
    return type(g:solcore_lsp_command) == v:t_list ? copy(g:solcore_lsp_command) : [g:solcore_lsp_command]
  endif
  if exists('g:solcore_path') && !empty(g:solcore_path)
    return [g:solcore_path, 'lsp']
  endif
  if !empty($SOLCORE_PATH)
    return [$SOLCORE_PATH, 'lsp']
  endif
  if !empty($SOLCORE_LSP_SERVER)
    return [$SOLCORE_LSP_SERVER]
  endif
  let windows = has('win32') || has('win64')
  let name = windows ? 'solcore.exe' : 'solcore'
  let cargo = empty($CARGO_HOME) ? expand('~/.cargo') : $CARGO_HOME
  let directories = [cargo . '/bin', expand('~/.local/bin')] + split($PATH, windows ? ';' : ':')
  for directory in directories
    if directory !~# '^/' && !(windows && directory =~# '^\%([A-Za-z]:[/\\]\|\\\\\)')
      continue
    endif
    let candidate = directory . '/' . name
    if executable(candidate)
      return [candidate, 'lsp']
    endif
  endfor
  throw 'Solcore compiler not found; install Solcore or set g:solcore_path'
endfunction
