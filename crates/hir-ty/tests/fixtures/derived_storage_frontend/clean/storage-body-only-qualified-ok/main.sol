pragma solcore;

import * from std;
import types;

function touchBox() {
    let size: word = StorageSize.size(@types.Box<uint256>);
    let slot: storage<types.Box<uint256>> = storage(0);
    let value: types.Box<uint256> = CanStore.load(slot);
}
