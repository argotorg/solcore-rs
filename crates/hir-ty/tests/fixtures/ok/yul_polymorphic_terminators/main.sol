pragma solcore;

function viaStop<a>() returns (a) {
    assembly {
        stop()
    }
}

function viaInvalid<a>() returns (a) {
    assembly {
        invalid()
    }
}

function viaSelfdestruct<a>(beneficiary: word) returns (a) {
    assembly {
        selfdestruct(beneficiary)
    }
}

function viaRevert<a>() returns (a) {
    assembly {
        revert(0, 0)
    }
}

function useWord(value: word) {}

contract Terminators {
    function main() public {
        useWord(viaStop());
        useWord(viaInvalid());
        useWord(viaSelfdestruct(0));
        useWord(viaRevert());
    }
}
