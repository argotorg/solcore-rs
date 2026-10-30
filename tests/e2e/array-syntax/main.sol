pragma solcore;

// `T[]` follows Solidity: a contract field `T[]` is a storage array
// (`array<T>`), while locals, parameters and returns are memory arrays
// (`memory<DynArray<T>>`). Location wrappers take array tags:
// `calldata<T[]>` is `calldata<array<T>>`, `mapping(K => T[])` maps to storage
// arrays, and suffixes nest (`T[][]`). As in Solidity, `a.length` reads the
// length and `push`/`pop` are method calls on any array expression.
import * from std;
import * from std.dispatch;

function sumAll(xs: uint256[]) returns (uint256) {
    let total: uint256 = 0;
    for (let i: uint256 = 0; i < xs.length; i += 1) {
        total += xs[i];
    }
    return total;
}

function pair(a: uint256, b: uint256) returns (uint256[]) {
    return [a, b];
}

// A struct field named `length` takes priority over the array length.
struct Span {
    length: uint256;
}

contract ArraySyntax {
    xs: uint256[];
    grid: uint256[][];
    owners: mapping(address => uint256[]);

    // #[send(5)]
    // #[send(6)]
    function push(v: uint256) public {
        xs.push(v);
    }

    // #[() -> 2]
    function count() public returns (uint256) {
        return xs.length;
    }

    // #[(1) -> 6]
    function at(i: uint256) public returns (uint256) {
        return xs[i];
    }

    // #[(3, 4) -> 10]
    function local(a: uint256, b: uint256) public returns (uint256) {
        let m: uint256[] = [a, b, a];
        return sumAll(m);
    }

    // #[(3, 4) -> 4]
    function returned(a: uint256, b: uint256) public returns (uint256) {
        let m: uint256[] = pair(a, b);
        return m[1];
    }

    // #[() -> 5]
    function nested() public returns (uint256) {
        let rows: uint256[][] = [[uint256(1), uint256(2)], [uint256(3)]];
        return rows[0][1] + rows[1][0];
    }

    // #[(7) -> 7]
    function storageGrid(v: uint256) public returns (uint256) {
        grid.setLength(uint256(1));
        grid[0].push(v);
        return grid[0][0] + grid[0].length - grid.length;
    }

    // #[(0x1111111111111111111111111111111111111111, 9) -> 9]
    function own(who: address, v: uint256) public returns (uint256) {
        owners[who].push(v);
        return owners[who][0];
    }

    // #[(4) -> 4]
    function callResultLength(n: uint256) public returns (uint256) {
        return pair(n, n).length + new uint256[](n).length - 2;
    }

    // #[(6) -> 6]
    function structLength(n: uint256) public returns (uint256) {
        let span = Span(n);
        return span.length;
    }
}
