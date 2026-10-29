pragma solcore;

function addW(x: Word, y: Word) returns (Word) {
    let res: Word;
    assembly {
        res := add(x, y)
    }
    return res;
}

trait Enum<a> {
    function fromEnum(x: a) returns (Word);
}

enum Color {
    R,
    G,
    B
}

impl Enum<Color> {
    function fromEnum(c: Color) returns (Word) {
        match (c) {
            case R {
                return 1;
            }
            case Color.G {
                return 2;
            }
            case Color.B {
                return 3;
            }
        }
    }
}

enum Bool {
    False,
    True
}

impl Enum<Bool> {
    function fromEnum(b: Bool) returns (Word) {
        match (b) {
            case False {
                return 0;
            }
            case Bool.True {
                return 1;
            }
        }
    }
}
enum FromEnumToken<a> {
    FromEnumToken
}

trait Invokable<self, args, ret> {
    function invoke(s: self, a: args) returns (ret);
}

impl<a> Invokable<FromEnumToken<a>, a, Word> where a: Enum {
    function invoke(fet: FromEnumToken<a>, arg: a) returns (Word) {
        return fromEnum(arg);
    }
}
contract RGB {
    function main() public returns (Word) {
        /*
  let x = fromEnum(Color.B);
  let y = fromEnum(Bool.True);
  */

        let fetC = FromEnumToken;
        let fetB = FromEnumToken;
        let x = invoke(fetC, Color.B);
        let y = invoke(fetB, Bool.True);
        return addW(x, y);
    }
}
