pragma solcore;

import * from abi;
import { Box } from types;
import { keepCompetitorReachable } from competitor;

function main(p: Proxy<Box>) returns (word) {
    return ABIAttribs.headSize(p);
}
