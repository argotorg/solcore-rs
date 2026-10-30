pragma solcore;

enum Tree<a> {
    Leaf,
    Node(Tree<a>, a, Tree<a>)
}
