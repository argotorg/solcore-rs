pragma solcore;

import lib;
import { Proxy as Renamed } from lib;

type Word = word;
type Witness = lib.Proxy<word>;
enum Proxy<t> {
    Proxy
}

function needsWord(p: Proxy<Word>) {}

function inferred() {
    let proxy = @Word;
    needsWord(proxy);
}

function local() returns (Proxy<word>) {
    return @word;
}

function qualified() returns (lib.Proxy<word>) {
    return @word;
}

function aliased() returns (Witness) {
    return @word;
}

function renamed() returns (Renamed<word>) {
    return @word;
}
