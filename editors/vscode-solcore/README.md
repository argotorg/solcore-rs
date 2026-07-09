# Solcore editor grammar

This directory contains the reusable editor definition for Solcore `.solc`
files. The playground imports the TextMate grammar directly from
`syntaxes/solcore.tmLanguage.json`, so local editor support and browser syntax
highlighting share the same source definition.

The package is shaped like a small VS Code extension:

- `syntaxes/solcore.tmLanguage.json` provides TextMate scopes.
- `language-configuration.json` provides comments, brackets, auto-close pairs,
  indentation, folding markers, and the Solcore word pattern.
- `package.json` wires the `.solc` extension to the grammar and configuration.

For local VS Code development, open this directory as an extension development
host or symlink/copy it into your VS Code extensions directory. Other editors
that consume TextMate grammars can use `syntaxes/solcore.tmLanguage.json`
directly.
