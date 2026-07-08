pub(crate) struct ScopeStack<T> {
    scopes: Vec<T>,
    empty_message: &'static str,
}

impl<T> ScopeStack<T> {
    pub(crate) fn new_root(root: T) -> Self {
        Self::new_root_with_message(root, "scope stack is never empty")
    }

    pub(crate) fn new_root_with_message(root: T, empty_message: &'static str) -> Self {
        Self {
            scopes: vec![root],
            empty_message,
        }
    }

    pub(crate) fn push(&mut self, scope: T) {
        self.scopes.push(scope);
    }

    pub(crate) fn pop(&mut self) -> T {
        self.scopes.pop().expect(self.empty_message)
    }

    pub(crate) fn last_mut(&mut self) -> &mut T {
        self.scopes.last_mut().expect(self.empty_message)
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.scopes.iter()
    }
}
