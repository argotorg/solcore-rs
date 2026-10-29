pragma solcore;

import * from std;
import { Box } from types;

function touchBox() {
    let size: word = StorageSize.size(@Box<uint256>);
    let slot: storage<Box<uint256>> = storage(0);
    let value: Box<uint256> = CanStore.load(slot);
}
