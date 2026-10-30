pragma solcore;

//~ layout: unsupported-recursive
enum Chain {
    End(word),
    Link(word, Chain)
}

function inspect(value: Chain) returns (word) {
    assembly {
        mstore(96, 1)
    }
    match (value) {
        case Chain.End(payload) {
            return payload;
        }
        case Chain.Link(head, tail) {
            return head;
        }
    }
}

contract Recursive {
    function main() {
        let input: word;
        assembly {
            input := calldataload(0)
        }
        let result: word = inspect(Chain.End(input));
        assembly {
            mstore(0, result)
            return (0, 32)
        }
    }
}
