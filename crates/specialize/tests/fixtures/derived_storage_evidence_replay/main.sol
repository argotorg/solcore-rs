import * from storage_support;
import { Box } from types;

function main(r: storage<Box<word>>, v: Box<word>) returns (Box<word>) {
    let slots = StorageSize.size(@Box<word>);
    CanStore.store(r, v);
    return CanStore.load(r);
}
