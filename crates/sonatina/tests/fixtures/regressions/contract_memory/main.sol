pragma solcore;

contract MemoryContract {
    function main() returns (word) {
        let result: word;
        assembly {
            mstore(0, 42)
            result := mload(0)
        }
        return result;
    }
}

//~ sonatina: contains "object @MemoryContractDeploy"
//~ sonatina: contains "embed .runtime as &MemoryContract"
//~ sonatina: contains "sym_addr &MemoryContract"
//~ sonatina: contains "sym_size &MemoryContract"
//~ sonatina: contains "evm_mstore "
//~ sonatina: contains "evm_mload "
//~ sonatina: compile O0
