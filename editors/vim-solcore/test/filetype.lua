-- Run from the repository root:
-- nvim --headless -u NONE -l editors/vim-solcore/test/filetype.lua
-- Use only this checkout and Neovim's runtime, not an installed package.
vim.opt.packpath = { vim.env.VIMRUNTIME }
vim.opt.runtimepath = { vim.fn.getcwd() .. "/editors/vim-solcore", vim.env.VIMRUNTIME }
vim.cmd("filetype on")
local directory = vim.fn.tempname()
vim.fn.mkdir(directory, "p")

local function filetype_of(name, lines)
  local path = directory .. "/" .. name
  vim.fn.writefile(lines, path)
  vim.cmd.edit(path)
  return vim.bo.filetype
end

-- The built-in detection maps *.sol to solidity; only the pragma overrides it.
assert(filetype_of("first.sol", { "pragma solcore;", "", "contract C {}" }) == "solcore")
assert(filetype_of("commented.sol", { "// SPDX-License-Identifier: MIT", "/* header", " */ pragma solcore;" })
  == "solcore")
assert(filetype_of("solidity.sol", { "// SPDX-License-Identifier: MIT", "pragma solidity ^0.8.0;" })
  == "solidity")
assert(filetype_of("misplaced.sol", { "import * from std;", "pragma solcore;" }) == "solidity")

-- A new file becomes Solcore once it is written with the pragma.
vim.cmd.edit(directory .. "/new.sol")
assert(vim.bo.filetype == "solidity")
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "pragma solcore;" })
vim.cmd.write()
assert(vim.bo.filetype == "solcore")

vim.fn.delete(directory, "rf")
print("Solcore Neovim filetype detection tests passed")
