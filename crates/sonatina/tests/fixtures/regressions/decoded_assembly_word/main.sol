pragma solcore;

function main() returns (word) {
    let x: word;
    assembly {
        x := "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n"
    }
    return x;
}

//~ sonatina: contains "4540866244600635114649842549360310111892940575123159374096375843447573711370"
