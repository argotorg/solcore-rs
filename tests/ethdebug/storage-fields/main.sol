pragma solcore;
import * from std;
import * from std.dispatch;

enum Shape {
    Dot(uint256),
    Seg(uint256, uint256),
    Nothing
}

enum Pair<a, b> {
    Pair(a, b)
}

function first<a, b>(p: Pair<a, b>) returns (a) {
    match (p) {
        case Pair(x, y) {
            return x;
        }
    }
}

function extent(s: Shape) returns (uint256) {
    match (s) {
        case Shape.Dot(d) {
            return d;
        }
        case Shape.Seg(lo, hi) {
            return hi - lo;
        }
        case Shape.Nothing {
            return uint256(0);
        }
    }
}

contract StorageFields {
    count: uint256;
    untouched: uint256;
    label: string;
    balances: mapping(address => uint256);

    function main() public returns (uint256) {
        count = uint256(1);
        let total: uint256 = count + uint256(10);
        if (count == uint256(1)) {
            let bonus: uint256 = uint256(5);
            total = total + bonus;
        }
        let total: uint256 = total + uint256(1);
        let seed: comptime<word> = 7;
        let f = lam (v: uint256) -> uint256 {
            return v + total;
        };
        let w: word = first(Pair(toWord(total), toWord(count)));
        let flag: bool = first(Pair(count == uint256(1), total));
        let width: uint256 = extent(Shape.Seg(count, total));
        balances[caller()] = total;
        count = f(uint256(0));
        return count;
    }
}

function caller() returns (address) {
    let res: word;
    assembly {
        res := caller()
    }
    return address(res);
}
