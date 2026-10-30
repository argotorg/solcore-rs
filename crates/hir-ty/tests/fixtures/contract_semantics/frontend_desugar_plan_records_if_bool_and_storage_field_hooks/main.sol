pragma solcore;

contract C {
    flag: word;

    function f() public returns (word) {
        if (true) {
            flag = 1;
        } else {
            return flag;
        }
    }
}
