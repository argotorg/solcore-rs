pragma solcore;

import * from std;
import * from std.dispatch;
import { callvalue } from std.opcodes;

// TODO: Should use uint64.
// Assumes 64-bit input.
function to_little_endian_64(v: uint256) returns (memory<bytes>) {
    let res: word = allocate_memory(32 + 8);
    let value: word = Typedef.rep(v);
    assembly {
        mstore(res, 8)
        mstore8(add(res, 32), and(value, 0xff))
        mstore8(add(res, 33), and(shr(8, value), 0xff))
        mstore8(add(res, 34), and(shr(16, value), 0xff))
        mstore8(add(res, 35), and(shr(24, value), 0xff))
        mstore8(add(res, 36), and(shr(32, value), 0xff))
        mstore8(add(res, 37), and(shr(40, value), 0xff))
        mstore8(add(res, 38), and(shr(48, value), 0xff))
        mstore8(add(res, 39), and(shr(56, value), 0xff))
    }
    return memory(res);
}

// No constants are supported yet, using this as a workaround.
// Defining variables outside of contract/function is not supported.
function DEPOSIT_CONTRACT_TREE_DEPTH() returns (uint256) {
    return 32;
}

function MAX_DEPOSIT_COUNT() returns (uint256) {
    // uint constant MAX_DEPOSIT_COUNT = 2**DEPOSIT_CONTRACT_TREE_DEPTH - 1;
    // TODO: Could use Bounded(uint32).maxVal()
    return 0xFFFFFFFF;
}

// #[scenario("depositcontract")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x43ca34020000000000000000000000000000000000000000000000000000000000000000, label = "get_zero_hash(0)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x43ca34020000000000000000000000000000000000000000000000000000000000000001, label = "get_zero_hash(1)") -> success(0xf5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b)]
// #[raw(0x43ca34020000000000000000000000000000000000000000000000000000000000000002, label = "get_zero_hash(2)") -> success(0xdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71)]
// #[raw(0x43ca34020000000000000000000000000000000000000000000000000000000000000003, label = "get_zero_hash(3)") -> success(0xc78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c)]
// #[raw(0x43ca34020000000000000000000000000000000000000000000000000000000000000004, label = "get_zero_hash(4)") -> success(0x536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c)]
// #[raw(0x43ca3402000000000000000000000000000000000000000000000000000000000000000f, label = "get_zero_hash(15)") -> success(0xd49a7502ffcfb0340b1d7885688500ca308161a7f96b62df9d083b71fcc8f2bb)]
// #[raw(0x43ca3402000000000000000000000000000000000000000000000000000000000000001f, label = "get_zero_hash(31)") -> success(0x985e929f70af28d0bdd1a90a808f977f597c7c778c489e98d3bd8910d31ac0f7)]
// #[raw(0xc5f2892f, label = "get_deposit_root()") -> success(0xd70a234731285c6804c2a4f56711ddb8c82c99740f207854891028af34e27e5e)]
// #[raw(0x621fd130, label = "get_deposit_count()") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x22895118000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000e00000000000000000000000000000000000000000000000000000000000000120d2b73e995b4c94bf83bcbf58cb9dede2c5e0211ec61eeb09c43aeaad97ce02620000000000000000000000000000000000000000000000000000000000000030abe405f4ca553d02da780b187e15c767fc89d4f9a707bd54c98a8d8e401124dedc6bc14dcdf0ed690e9ec424deae49a50000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200100000000000000000000007e2a2fa2a064f693f0a55c5639476d913ff12d0500000000000000000000000000000000000000000000000000000000000000608927fa063b70c19a28c3c78aa19bbb2e7b971fc1b8de2033930952a11ef17709348cc2cc14ae810ab249a1fe39e959470b850608afc299811c5cce190f92dd79529ba11846c138073ae200782476c8e5e4c6ea3b5938a16398835e4964c6bac0, value = 32000000000000000000, label = "deposit(bytes,bytes,bytes,bytes32)") -> success(0x)]
// #[raw(0xc5f2892f, label = "get_deposit_root()") -> success(0xd3014e96ac4ef15ccbac8047549cac09ad687f6169cba901b92c8875a62ca917)]
// #[raw(0x621fd130, label = "get_deposit_count()") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000080100000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x22895118000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000e00000000000000000000000000000000000000000000000000000000000000120d2b73e995b4c94bf83bcbf58cb9dede2c5e0211ec61eeb09c43aeaad97ce02620000000000000000000000000000000000000000000000000000000000000030abe405f4ca553d02da780b187e15c767fc89d4f9a707bd54c98a8d8e401124dedc6bc14dcdf0ed690e9ec424deae49a50000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200100000000000000000000007e2a2fa2a064f693f0a55c5639476d913ff12d0500000000000000000000000000000000000000000000000000000000000000608927fa063b70c19a28c3c78aa19bbb2e7b971fc1b8de2033930952a11ef17709348cc2cc14ae810ab249a1fe39e959470b850608afc299811c5cce190f92dd79529ba11846c138073ae200782476c8e5e4c6ea3b5938a16398835e4964c6bac0, label = "deposit(bytes,bytes,bytes,bytes32)") -> revert(0xbdfc7472)]
// #[raw(0xc5f2892f, label = "get_deposit_root()") -> success(0xd3014e96ac4ef15ccbac8047549cac09ad687f6169cba901b92c8875a62ca917)]
// #[raw(0x621fd130, label = "get_deposit_count()") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000080100000000000000000000000000000000000000000000000000000000000000)]
contract DepositContract {
    deposit_count: uint256;
    // TODO: use fixed-size arrays of DEPOSIT_CONTRACT_TREE_DEPTH() length
    branch: array<bytes32>;
    zero_hashes: array<bytes32>;

    constructor() {
        // Dynamic storage arrays start empty and indexed access is bounds-checked,
        // so grow both to DEPOSIT_CONTRACT_TREE_DEPTH() elements before indexing
        // them at fixed positions. New slots are zero-initialised, which matches
        // the empty sparse Merkle tree these arrays represent.
        // Alternatively, `.push(..)` could be used, but this saves sstores.
        Array.setLength(branch, DEPOSIT_CONTRACT_TREE_DEPTH());
        Array.setLength(zero_hashes, DEPOSIT_CONTRACT_TREE_DEPTH());

        // Compute hashes in empty sparse Merkle tree
        for (let height = 0; height<(DEPOSIT_CONTRACT_TREE_DEPTH() - 1); height += 1) {
                zero_hashes[height + 1] = sha256(concat(zero_hashes[height], zero_hashes[height]));
            }
        }

        // TODO: this is for testing only
        function get_zero_hash(index: uint256) public returns (bytes32) {
            return zero_hashes[index];
        }

        function get_deposit_root() public returns (bytes32) {
            let node: bytes32;
            let size = deposit_count;
            for (let height = 0; height < DEPOSIT_CONTRACT_TREE_DEPTH(); height += 1) {
                if ((size & 1) == 1) {
                    node = sha256(concat(branch[height], node));
                } else {
                    node = sha256(concat(node, zero_hashes[height]));
                }
                size /= 2;
            }
            return sha256(concat(
                    concat(
                        node,
                        to_little_endian_64(deposit_count)
                    ),
                    empty(24)
                ));
        }

        function get_deposit_count() public returns (memory<bytes>) {
            return to_little_endian_64(deposit_count);
        }

        // TODO: once string literals are properly supported, change errors to messages
        // matching the deposit contract, full 100% identical behaviour.
        function deposit(pubkey: memory<bytes>, withdrawal_credentials: memory<bytes>, signature: memory<bytes>, deposit_data_root: bytes32) public payable {
            // Extended ABI length checks since dynamic types are used.
            require(MemorySize.len(pubkey) == 48, Error(0x9ca717ed)); // InvalidPubkeyLength()
            require(MemorySize.len(withdrawal_credentials) == 32, Error(0x3debbf1e)); // InvalidWithdrawalCredentialsLength()
            require(MemorySize.len(signature) == 96, Error(0x4be6321b)); // InvalidSignatureLength()

            // Check deposit amount
            // >= 1 ether
            require(callvalue() >= 1000000000000000000, Error(0xbdfc7472)); // DepositValueTooLow()
            // % 1 gwei == 0
            require((callvalue() % 1000000000) == 0, Error(0x9c7417e9)); // DepositValueNotMultipleOfOneGwei()

            let deposit_amount = callvalue() / 1000000000; // 1 gwei
            // <= type(uint64).max
            require(deposit_amount <= 0xffffffffffffffff, Error(0x2aa66734)); // DepositValueTooHigh()

            let amount: memory<bytes> = to_little_endian_64(uint256(deposit_amount));
            // TODO: emit DepositEvent
            /*
    event DepositEvent(
        bytes pubkey,
        bytes withdrawal_credentials,
        bytes amount,
        bytes signature,
        bytes index
    );

        emit DepositEvent(
            pubkey,
            withdrawal_credentials,
            amount,
            signature,
            to_little_endian_64(uint64(deposit_count))
        );
    */
            assembly {
                // TODO: need to ABI-encode all the arguments
                log1(0, 0, 0x649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5)
            }

            // Compute deposit data root (`DepositData` hash tree root)
            let pubkey_root = sha256(concat(pubkey, empty(16)));
            let signature_root = sha256(concat(
                    sha256(truncate(signature, 64)),
                    sha256(concat(slice_(signature, 64), empty(32)))
                ));
            let node = sha256(concat(
                    sha256(concat(pubkey_root, withdrawal_credentials)),
                    sha256(concat(concat(amount, empty(24)), signature_root))
                ));

            // Verify computed and expected deposit data roots match
            require(node == deposit_data_root, Error(0x2ec2f183)); // ReconstructedDepositDataMismatch()

            // Avoid overflowing the Merkle tree (and prevent edge case in computing `branch`)
            require(deposit_count < MAX_DEPOSIT_COUNT(), Error(0xef5ccf66)); // MerkleTreeFull()

            // Add deposit data root to Merkle tree (update a single `branch` node)
            deposit_count += 1;
            let size = deposit_count;
            for (let height = 0; height < DEPOSIT_CONTRACT_TREE_DEPTH(); height += 1) {
                if ((size & 1) == 1) {
                    branch[height] = node;
                    return ();
                }
                node = sha256(concat(branch[height], node));
                size /= 2;
            }

            // As the loop should always end prematurely with the `return` statement,
            // this code should be unreachable. We assert `false` just to be safe.
            assert(false);
        }

        function supportsInterface(interfaceId: bytes4) public returns (bool) {
            unimplemented();
            return false;
        }
    }
