pragma solcore;

function classify_word(comptime x: word) returns (word) {
    match (x) {
        case 0 {
            return 10;
        }
        default {
            return 20;
        }
    }
}

function classify_integer(comptime x: integer) returns (word) {
    match (x) {
        case 0 {
            return 10;
        }
        default {
            return 20;
        }
    }
}
