pragma solcore;

function main(s: string) returns (word) {
    match (s) {
        case "a" {
            return 1;
        }
        default {
            return 2;
        }
    }
}
