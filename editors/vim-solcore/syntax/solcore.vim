if exists('b:current_syntax')
  finish
endif

syntax case match

syntax keyword solcoreTodo TODO FIXME XXX NOTE contained

syntax region solcoreLineComment start=+//+ end=+$+ contains=solcoreTodo,@Spell
syntax region solcoreBlockComment start=+/\*+ end=+\*/+ contains=solcoreBlockComment,solcoreTodo,@Spell

syntax match solcoreEscape +\\[nt"\\]+ contained
syntax match solcoreInvalidEscape +\\.+ contained
syntax region solcoreString start=+"+ skip=+\\\\\|\\"+ end=+"+ contains=solcoreEscape,solcoreInvalidEscape

syntax match solcoreContractDeclaration #\v(^|[^[:alnum:]_-])contract\s+\zs[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#
syntax match solcoreFunctionDeclaration #\v(^|[^[:alnum:]_-])function\s+\zs[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#
syntax match solcoreTypeDeclaration #\v(^|[^[:alnum:]_-])(data|class|type)\s+\zs[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#
syntax match solcoreVariableDeclaration #\v(^|[^[:alnum:]_-])let\s+\zs[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#
syntax match solcorePragmaDeclaration #\v(^|[^[:alnum:]_-])pragma\s+\zs[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#

syntax match solcoreControlKeyword #\v(^|[^[:alnum:]_-])\zs(if|else|for|switch|case|default|match|return|leave|continue|break)\ze([^[:alnum:]_-]|$)#
syntax match solcoreDeclarationKeyword #\v(^|[^[:alnum:]_-])\zs(contract|import|export|as|let|data|class|forall|instance|type|function|constructor|fallback|assembly|pragma|lam)\ze([^[:alnum:]_-]|$)#
syntax match solcoreStorageModifier #\v(^|[^[:alnum:]_-])\zs(public|payable)\ze([^[:alnum:]_-]|$)#

syntax match solcoreBoolean #\v(^|[^[:alnum:]_-])\zs(true|false)\ze([^[:alnum:]_-]|$)#
syntax match solcoreWildcard #\v(^|[^[:alnum:]_-])\zs_\ze([^[:alnum:]_-]|$)#

syntax match solcorePrimitiveType #\v(^|[^[:alnum:]_-])\zs(word|bool|unit)\ze([^[:alnum:]_-]|$)#
syntax match solcoreTypeIdentifier #\v(^|[^[:alnum:]_-])\zs[A-Z][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*#

syntax match solcoreHexNumber #\v(^|[^[:alnum:]_])\zs0x[0-9a-fA-F]+\ze([^[:alnum:]_]|$)#
syntax match solcoreDecimalNumber #\v(^|[^[:alnum:]_])\zs[0-9]+\ze([^[:alnum:]_]|$)#

syntax match solcoreFunctionCall #\v[[:alpha:]][[:alnum:]_]*(-[[:alpha:]][[:alnum:]_]*)*\ze\s*\(#

syntax match solcoreOperator #:=#
syntax match solcoreOperator #+=#
syntax match solcoreOperator #-=#
syntax match solcoreOperator #\^=#
syntax match solcoreOperator #&=#
syntax match solcoreOperator #|=#
syntax match solcoreOperator #%=#
syntax match solcoreOperator #->#
syntax match solcoreOperator #=>#
syntax match solcoreOperator #==#
syntax match solcoreOperator #!=#
syntax match solcoreOperator #>=#
syntax match solcoreOperator #<=#
syntax match solcoreOperator #<#
syntax match solcoreOperator #>#
syntax match solcoreOperator #&&#
syntax match solcoreOperator #||#
syntax match solcoreOperator #!#
syntax match solcoreOperator #+#
syntax match solcoreOperator #-#
syntax match solcoreOperator #\*#
syntax match solcoreOperator #/#
syntax match solcoreOperator #%#
syntax match solcoreOperator #|#
syntax match solcoreOperator #&#
syntax match solcoreOperator #\^#
syntax match solcoreOperator #@#
syntax match solcoreOperator #?#
syntax match solcoreOperator #=#

syntax match solcoreDelimiter +[{}()[\].,:;]+

highlight default link solcoreTodo Todo
highlight default link solcoreLineComment Comment
highlight default link solcoreBlockComment Comment
highlight default link solcoreString String
highlight default link solcoreEscape SpecialChar
highlight default link solcoreInvalidEscape Error
highlight default link solcoreContractDeclaration Type
highlight default link solcoreFunctionDeclaration Function
highlight default link solcoreTypeDeclaration Type
highlight default link solcoreVariableDeclaration Identifier
highlight default link solcorePragmaDeclaration PreProc
highlight default link solcoreControlKeyword Conditional
highlight default link solcoreDeclarationKeyword Keyword
highlight default link solcoreStorageModifier StorageClass
highlight default link solcoreBoolean Boolean
highlight default link solcoreWildcard Special
highlight default link solcorePrimitiveType Type
highlight default link solcoreTypeIdentifier Type
highlight default link solcoreHexNumber Number
highlight default link solcoreDecimalNumber Number
highlight default link solcoreFunctionCall Function
highlight default link solcoreOperator Operator
highlight default link solcoreDelimiter Delimiter

let b:current_syntax = 'solcore'
