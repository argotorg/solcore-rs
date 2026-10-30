pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

enum Pair {
    Pair(uint256, uint256)
}

impl StorageCopy<Pair> {
    function copySlot(dst: storage<Pair>, src: storage<Pair>) {
        let value: Pair = CanStore.load(src);
        CanStore.store(dst, value);
    }
}

impl Add<Pair> {
    function add(x: Pair, y: Pair) returns (Pair) {
        match (x, y) {
            case (Pair(a, b), Pair(c, d)) {
                return Pair(a + c, b + d);
            }
        }
    }
}

contract StorageArrayLayout {
    pairs: array<Pair>;
    source: array<Pair>;
    grid: array<array<uint256>>;
    sourceGrid: array<array<uint256>>;
    cube: array<array<array<uint256>>>;
    numbers: array<uint256>;
    ticks: uint256;

    function nextIndex() returns (uint256) {
        ticks += 1;
        return 1;
    }

    // #[() -> 22]
    function widePushAndIndex() public returns (uint256) {
        ArrayPush.push(pairs, Pair(uint256(11), uint256(22)));
        ArrayPush.push(pairs, Pair(uint256(33), uint256(44)));
        match (pairs[0]) {
            case Pair(a, b) {
                return b;
            }
        }
    }

    // #[() -> 33]
    function wideAssignment() public returns (uint256) {
        ArrayPush.push(pairs, Pair(uint256(11), uint256(22)));
        ArrayPush.push(pairs, Pair(uint256(33), uint256(44)));
        pairs[0] = Pair(uint256(55), uint256(66));
        match (pairs[1]) {
            case Pair(a, b) {
                return a;
            }
        }
    }

    // #[() -> (22, 33, 49, 1)]
    function wideCompoundAssignment() public returns (uint256, uint256, uint256, uint256) {
        ArrayPush.push(pairs, Pair(uint256(11), uint256(22)));
        ArrayPush.push(pairs, Pair(uint256(33), uint256(44)));
        ticks = 0;
        pairs[nextIndex()] += Pair(uint256(0), uint256(5));
        match (pairs[0], pairs[1]) {
            case (Pair(a, b), Pair(c, d)) {
                return (b, c, d, ticks);
            }
        }
    }

    // #[() -> (11, 22, 0, 0)]
    function widePopAndGrow() public returns (uint256, uint256, uint256, uint256) {
        ArrayPush.push(pairs, Pair(uint256(11), uint256(22)));
        ArrayPush.push(pairs, Pair(uint256(33), uint256(44)));
        Array.pop(pairs);
        Array.setLength(pairs, uint256(2));
        match (pairs[0], pairs[1]) {
            case (Pair(a, b), Pair(c, d)) {
                return (a, b, c, d);
            }
        }
    }

    // #[() -> (11, 22, 33, 44)]
    function wideCopy() public returns (uint256, uint256, uint256, uint256) {
        ArrayPush.push(source, Pair(uint256(11), uint256(22)));
        ArrayPush.push(source, Pair(uint256(33), uint256(44)));
        pairs = source;
        source[0] = Pair(uint256(55), uint256(66));
        pairs = pairs;
        match (pairs[0], pairs[1]) {
            case (Pair(a, b), Pair(c, d)) {
                return (a, b, c, d);
            }
        }
    }

    // #[() -> (7, 9, 0, 0)]
    function wideCopyShrinkAndGrow() public returns (uint256, uint256, uint256, uint256) {
        ArrayPush.push(pairs, Pair(uint256(11), uint256(22)));
        ArrayPush.push(pairs, Pair(uint256(33), uint256(44)));
        ArrayPush.push(source, Pair(uint256(7), uint256(9)));
        pairs = source;
        Array.setLength(pairs, uint256(2));
        match (pairs[0], pairs[1]) {
            case (Pair(a, b), Pair(c, d)) {
                return (a, b, c, d);
            }
        }
    }

    // #[() -> 0]
    function nestedRegrowth() public returns (uint256) {
        Array.setLength(grid, uint256(1));
        ArrayPush.push(grid[0], uint256(7));
        Array.pop(grid);
        Array.setLength(grid, uint256(1));
        Array.setLength(grid[0], uint256(1));
        return grid[0][0];
    }

    // #[() -> 0]
    function deepRegrowth() public returns (uint256) {
        Array.setLength(cube, uint256(1));
        Array.setLength(cube[0], uint256(1));
        ArrayPush.push(cube[0][0], uint256(7));
        Array.setLength(cube, uint256(0));
        Array.setLength(cube, uint256(1));
        Array.setLength(cube[0], uint256(1));
        Array.setLength(cube[0][0], uint256(1));
        return cube[0][0][0];
    }

    // #[() -> (9, 0)]
    function nestedCopyShrinkAndGrow() public returns (uint256, uint256) {
        Array.setLength(grid, uint256(2));
        ArrayPush.push(grid[0], uint256(5));
        ArrayPush.push(grid[1], uint256(7));
        Array.setLength(sourceGrid, uint256(1));
        ArrayPush.push(sourceGrid[0], uint256(9));
        grid = sourceGrid;
        Array.setLength(grid, uint256(2));
        Array.setLength(grid[1], uint256(1));
        return (grid[0][0], grid[1][0]);
    }

    // #[() -> (9, 0, 0)]
    function literalShrinkAndGrow() public returns (uint256, uint256, uint256) {
        numbers = [1, 2, 3];
        numbers = [9];
        Array.setLength(numbers, uint256(3));
        return (numbers[0], numbers[1], numbers[2]);
    }

    // #[() -> 2]
    function zeroSizedElements() public returns (uint256) {
        let units: storage<array<()>> = storage(100);
        ArrayPush.push(units, ());
        Array.pop(units);
        Array.setLength(units, uint256(2));
        return Length.length(units);
    }
}
