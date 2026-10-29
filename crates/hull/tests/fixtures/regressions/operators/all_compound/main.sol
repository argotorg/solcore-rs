pragma solcore;

import * from std;

contract C {
    function main() public returns (word) {
        let acc: word = 6;
        acc += 4;
        acc -= 3;
        acc *= 6;
        acc /= 2;
        acc %= 8;
        acc ^= 3;
        acc |= 9;
        acc &= 12;
        acc ~=;
        acc &= 15;
        return acc;
    }
}
