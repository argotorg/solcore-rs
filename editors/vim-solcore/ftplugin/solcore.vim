if exists('b:did_ftplugin')
  finish
endif
let b:did_ftplugin = 1

let b:undo_ftplugin = 'setlocal commentstring< comments< formatoptions< include< iskeyword< suffixesadd<'

setlocal commentstring=//\ %s
setlocal comments=s1:/*,mb:*,ex:*/,://
let &l:include = '^\s*\%(import\|export\)\s\+'
setlocal suffixesadd=.solc

setlocal formatoptions-=t
setlocal formatoptions+=croql

" Solcore identifiers can contain hyphen-separated segments.
setlocal iskeyword+=-
