" A `.sol` file is Solcore when it starts with `pragma solcore;`. Other `.sol`
" files keep the editor's default detection, which treats them as Solidity.
augroup solcore_filetype
  autocmd!
  autocmd BufNewFile,BufRead *.sol if solcore#declares_pragma() | set filetype=solcore | endif
  " A new file becomes Solcore once it is written with the pragma.
  autocmd BufWritePost *.sol
        \ if &filetype !=# 'solcore' && solcore#declares_pragma() | set filetype=solcore | endif
augroup END
