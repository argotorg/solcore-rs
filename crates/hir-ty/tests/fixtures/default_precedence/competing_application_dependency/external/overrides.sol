pragma solcore;

pragma no-patterson-condition Pick;
export { Pick, Ready };
trait Pick<a> {}
trait Ready<a> {}
default impl<a> Pick<a> where a: Ready {}
impl Ready<word> {}
