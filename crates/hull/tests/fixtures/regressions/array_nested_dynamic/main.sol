pragma solcore;

//~ hull: contains "LValueIdxAccess_lookup"
//~ hull: contains "storeBytesFromMemory"
//~ hull: contains "loadBytesFromStorage"
//~ hull: contains "frombool"
//~ hull: contains "tobool"

import * from std;

contract CollectionArray {
    flags: array<bool>;
    grid: array<array<uint256>>;
    names: array<string>;
    backup: array<string>;

    function main() returns (uint256) {
        Array.setLength(flags, uint256(0));
        ArrayPush.push(flags, true);
        let flag: bool = flags[uint256(0)];

        Array.setLength(grid, uint256(1));
        ArrayPush.push(grid[uint256(0)], uint256(7));
        grid[uint256(0)][uint256(0)] = uint256(9);
        let row: storage<array<uint256>> = grid[uint256(0)];
        ArrayPush.push(row, uint256(11));

        let s: memory<string> = "hello";
        ArrayPush.push(names, s);
        names[uint256(0)] = s;
        let loaded: memory<string> = names[uint256(0)];
        backup = names;
        let copied: memory<string> = backup[uint256(0)];

        if (flag) {
            return row[uint256(1)] + uint256(strlen(loaded)) + uint256(strlen(copied));
        }
        return uint256(0);
    }
}
