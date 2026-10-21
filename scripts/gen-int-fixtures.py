#!/usr/bin/env python3
"""Generate the std.signed / std.unsigned E2E fixtures.

Writes tests/e2e/{signed,unsigned}-integers-{narrow,wide}/main.sol. Every
expectation comes from a model of Solidity 0.8 checked arithmetic: `+ - *`
outside the type's range and `MIN / -1` revert with Panic(0x11), division or
modulo by zero reverts with Panic(0x12), bitwise operations never revert.
Re-run after changing the semantics or the cases below:

    scripts/gen-int-fixtures.py

The generated sources are then formatted with `solcore-fmt`, like every other
E2E fixture.
"""

import subprocess
from pathlib import Path

M = 1 << 256
MASK = M - 1
PANIC_SELECTOR = "4e487b71"
OVERFLOW = 0x11
DIVISION_BY_ZERO = 0x12


class Panic:
    def __init__(self, code):
        self.code = code


def word(v):
    return v % M


def signed_of(w):
    return w - M if w >> 255 else w


def trunc_div(a, b):
    q = abs(a) // abs(b)
    return -q if (a < 0) != (b < 0) else q


def trunc_mod(a, b):
    r = abs(a) % abs(b)
    return -r if a < 0 else r


def lit(v):
    """Directive literal for a word: decimal when small, else hex."""
    v = word(v)
    return str(v) if v < 1 << 64 else hex(v)


def outcome(result):
    if isinstance(result, Panic):
        return f"revert(0x{PANIC_SELECTOR}{result.code:064x})"
    if isinstance(result, tuple):
        return "(" + ", ".join(lit(r) for r in result) + ")"
    return lit(result)


def cmp_mask(lt, gt, eq):
    return lt | (gt << 1) | ((lt or eq) << 2) | ((gt or eq) << 3) | (eq << 4) | ((not eq) << 5)


def directive(args, result):
    return f"  // #[({', '.join(lit(a) for a in args)}) -> {outcome(result)}]"


def binary(name, ty, op_src, cases):
    lines = [directive(c, r) for c, r in cases]
    lines += [
        f"  function {name}(a: uint256, b: uint256) public returns (uint256) {{",
        f"    let x: {ty} = Typedef.abs(Typedef.rep(a));",
        f"    let y: {ty} = Typedef.abs(Typedef.rep(b));",
        f"    return uint256(Typedef.rep({op_src}));",
        "  }",
    ]
    return lines


def unary(name, ty, op_src, cases):
    lines = [directive(c, r) for c, r in cases]
    lines += [
        f"  function {name}(a: uint256) public returns (uint256) {{",
        f"    let x: {ty} = Typedef.abs(Typedef.rep(a));",
        f"    return uint256(Typedef.rep({op_src}));",
        "  }",
    ]
    return lines


def compare(name, ty, cases):
    lines = [directive(c, r) for c, r in cases]
    lines += [
        f"  function {name}(a: uint256, b: uint256) public returns (uint256) {{",
        f"    let x: {ty} = Typedef.abs(Typedef.rep(a));",
        f"    let y: {ty} = Typedef.abs(Typedef.rep(b));",
        "    let r: uint256 = uint256(0);",
        "    if (x < y) { r = r + uint256(1); }",
        "    if (x > y) { r = r + uint256(2); }",
        "    if (x <= y) { r = r + uint256(4); }",
        "    if (x >= y) { r = r + uint256(8); }",
        "    if (x == y) { r = r + uint256(16); }",
        "    if (x != y) { r = r + uint256(32); }",
        "    return r;",
        "  }",
    ]
    return lines


def bounds(name, ty, lo, hi):
    return [
        directive((), (lo, hi)),
        f"  function {name}() public returns (uint256, uint256) {{",
        f"    let lo: {ty} = Bounded.minVal();",
        f"    let hi: {ty} = Bounded.maxVal();",
        "    return (uint256(Typedef.rep(lo)), uint256(Typedef.rep(hi)));",
        "  }",
    ]


def signed_section(bits):
    ty, sfx = f"int{bits}", f"I{bits}"
    lo, hi = -(1 << (bits - 1)), (1 << (bits - 1)) - 1

    def checked(v):
        return word(v) if lo <= v <= hi else Panic(OVERFLOW)

    def div(a, b):
        if b == 0:
            return Panic(DIVISION_BY_ZERO)
        return checked(trunc_div(a, b))

    def mod(a, b):
        if b == 0:
            return Panic(DIVISION_BY_ZERO)
        return word(trunc_mod(a, b))

    def bits_op(f):
        # Two's complement bitwise ops on in-range values stay in range.
        return lambda a, b: word(f(a, b))

    def cases(f, pairs):
        return [((word(a), word(b)), f(a, b)) for a, b in pairs]

    def cmp(a, b):
        return cmp_mask(a < b, a > b, a == b)

    out = [f"  // ---- {ty} ----"]
    out += binary(f"add{sfx}", ty, "x + y", cases(lambda a, b: checked(a + b), [
        (1, 2), (-5, 3), (hi, 0), (lo, hi), (hi, 1), (lo, -1)]))
    out += binary(f"sub{sfx}", ty, "x - y", cases(lambda a, b: checked(a - b), [
        (3, 5), (-3, -5), (-1, hi), (lo, 0), (lo, 1), (hi, -1)]))
    out += binary(f"mul{sfx}", ty, "x * y", cases(lambda a, b: checked(a * b), [
        (-7, 6), (-7, -6), (lo, 1), (0, lo), (hi, 2), (lo, -1)]))
    out += binary(f"div{sfx}", ty, "x / y", cases(div, [
        (-7, 2), (7, -2), (-8, -2), (lo, 1), (lo, -1), (5, 0)]))
    out += binary(f"mod{sfx}", ty, "x % y", cases(mod, [
        (7, 3), (-7, 3), (7, -3), (lo, -1), (5, 0)]))
    out += binary(f"and{sfx}", ty, "x & y", cases(bits_op(lambda a, b: a & b), [
        (-1, 15), (-16, 12)]))
    out += binary(f"or{sfx}", ty, "x | y", cases(bits_op(lambda a, b: a | b), [
        (-16, 5), (lo, hi)]))
    out += binary(f"xor{sfx}", ty, "x ^ y", cases(bits_op(lambda a, b: a ^ b), [
        (-1, 5), (lo, -1)]))
    out += unary(f"not{sfx}", ty, "~x", [((word(a),), word(~a)) for a in (0, -1, hi)])
    out += compare(f"cmp{sfx}", ty, cases(cmp, [(-1, 0), (0, -1), (-3, -3), (lo, hi)]))
    out += bounds(f"bounds{sfx}", ty, word(lo), word(hi))
    return out


def unsigned_section(bits):
    ty, sfx = f"uint{bits}", f"U{bits}"
    hi = (1 << bits) - 1
    half = 1 << (bits // 2)

    def checked(v):
        return v if 0 <= v <= hi else Panic(OVERFLOW)

    def div(a, b):
        return Panic(DIVISION_BY_ZERO) if b == 0 else a // b

    def mod(a, b):
        return Panic(DIVISION_BY_ZERO) if b == 0 else a % b

    def cases(f, pairs):
        return [((a, b), f(a, b)) for a, b in pairs]

    def cmp(a, b):
        return cmp_mask(a < b, a > b, a == b)

    out = [f"  // ---- {ty} ----"]
    out += binary(f"add{sfx}", ty, "x + y", cases(lambda a, b: checked(a + b), [
        (1, 2), (hi, 0), (hi, 1), (hi, hi)]))
    out += binary(f"sub{sfx}", ty, "x - y", cases(lambda a, b: checked(a - b), [
        (5, 3), (hi, hi), (0, 1), (3, 5)]))
    out += binary(f"mul{sfx}", ty, "x * y", cases(lambda a, b: checked(a * b), [
        (6, 7), (hi, 1), (0, hi), (hi, 2), (half, half)]))
    out += binary(f"div{sfx}", ty, "x / y", cases(div, [
        (7, 2), (hi, 1), (hi, hi), (5, 0)]))
    out += binary(f"mod{sfx}", ty, "x % y", cases(mod, [
        (7, 3), (hi, 10), (3, 7), (5, 0)]))
    out += binary(f"and{sfx}", ty, "x & y", cases(lambda a, b: a & b, [(hi, 15), (0xf0, 0x3c)]))
    out += binary(f"or{sfx}", ty, "x | y", cases(lambda a, b: a | b, [(0xf0, 0x0f), (hi - 1, 1)]))
    out += binary(f"xor{sfx}", ty, "x ^ y", cases(lambda a, b: a ^ b, [(hi, 1), (0xff, 0x0f)]))
    out += unary(f"not{sfx}", ty, "~x", [((a,), a ^ hi) for a in (0, hi, 0x0f)])
    out += compare(f"cmp{sfx}", ty, cases(cmp, [(1, 2), (2, 1), (hi, hi), (0, hi)]))
    out += bounds(f"bounds{sfx}", ty, 0, hi)
    return out


SIGNED_HEADER = """\
// std.signed: arithmetic, bitwise, comparison and bounds per width.
// Parameters and results are raw two's-complement words: each function
// rebuilds the intN value with Typedef.abs and returns Typedef.rep of the
// result. Negative operands are therefore passed sign-extended.
// Arithmetic is checked as in Solidity 0.8: overflow and MIN / -1 revert with
// Panic(0x11), division or modulo by zero with Panic(0x12); MIN % -1 is 0.
// cmp* returns a bitmask: lt=1, gt=2, le=4, ge=8, eq=16, ne=32.
// Generated by scripts/gen-int-fixtures.py.
import * from std;
import * from std.signed;
import * from std.dispatch;

contract SignedIntegers {
"""

UNSIGNED_HEADER = """\
// std.unsigned: arithmetic, bitwise, comparison and bounds per width.
// Parameters and results are raw words: each function rebuilds the uintN
// value with Typedef.abs and returns Typedef.rep of the result.
// Arithmetic is checked as in Solidity 0.8: overflow and underflow revert with
// Panic(0x11), division or modulo by zero with Panic(0x12).
// cmp* returns a bitmask: lt=1, gt=2, le=4, ge=8, eq=16, ne=32.
// Generated by scripts/gen-int-fixtures.py.
import * from std;
import * from std.unsigned;
import * from std.dispatch;

contract UnsignedIntegers {
"""

# One contract's dispatcher must stay within the default partial-evaluation
# fuel, so each family is split into narrow and wide widths.
FIXTURES = [
    ("signed-integers-narrow", SIGNED_HEADER, signed_section, (8, 16, 32)),
    ("signed-integers-wide", SIGNED_HEADER, signed_section, (64, 128, 256)),
    ("unsigned-integers-narrow", UNSIGNED_HEADER, unsigned_section, (8, 16, 32)),
    ("unsigned-integers-wide", UNSIGNED_HEADER, unsigned_section, (64, 128)),
]


def render(header, sections):
    body = []
    for section in sections:
        if body:
            body.append("")
        body += section
    return header + "\n".join(body) + "\n}\n"


def main():
    root = Path(__file__).resolve().parent.parent
    paths = []
    for name, header, section, widths in FIXTURES:
        src = render(header, [section(b) for b in widths])
        path = root / "tests" / "e2e" / name / "main.sol"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(src)
        paths.append(path)
        print(f"{path.relative_to(root)}: {src.count('#[')} directives")
    subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "solcore-fmt", "--", "--write", *paths],
        cwd=root,
        check=True,
    )


if __name__ == "__main__":
    main()
