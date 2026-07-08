use super::*;

/// Computes strongly connected components of a module graph.
///
/// Components are based on reference edges, not only imports, so export cycles
/// are represented in the same graph used by interface fixed points.
pub fn strongly_connected_components<'db>(graph: &ModuleGraph<'db>) -> Vec<Vec<ModuleId<'db>>> {
    let mut adjacency: FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>> = FxHashMap::default();
    for module in &graph.modules {
        adjacency.entry(*module).or_default();
    }
    for edge in &graph.reference_edges {
        adjacency.entry(edge.from).or_default().push(edge.to);
    }

    let mut state = TarjanState {
        next_index: 0,
        stack: Vec::new(),
        on_stack: FxHashSet::default(),
        indices: FxHashMap::default(),
        lowlinks: FxHashMap::default(),
        components: Vec::new(),
    };

    for module in &graph.modules {
        if !state.indices.contains_key(module) {
            strong_connect(*module, &adjacency, &mut state);
        }
    }

    state.components
}

struct TarjanState<'db> {
    next_index: usize,
    stack: Vec<ModuleId<'db>>,
    on_stack: FxHashSet<ModuleId<'db>>,
    indices: FxHashMap<ModuleId<'db>, usize>,
    lowlinks: FxHashMap<ModuleId<'db>, usize>,
    components: Vec<Vec<ModuleId<'db>>>,
}

fn strong_connect<'db>(
    module: ModuleId<'db>,
    adjacency: &FxHashMap<ModuleId<'db>, Vec<ModuleId<'db>>>,
    state: &mut TarjanState<'db>,
) {
    let index = state.next_index;
    state.next_index += 1;
    state.indices.insert(module, index);
    state.lowlinks.insert(module, index);
    state.stack.push(module);
    state.on_stack.insert(module);

    for target in adjacency.get(&module).into_iter().flatten() {
        if !state.indices.contains_key(target) {
            strong_connect(*target, adjacency, state);
            let target_low = state.lowlinks[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_low);
        } else if state.on_stack.contains(target) {
            let target_index = state.indices[target];
            let module_low = state.lowlinks.get_mut(&module).expect("module lowlink");
            *module_low = (*module_low).min(target_index);
        }
    }

    if state.lowlinks[&module] == state.indices[&module] {
        let mut component = Vec::new();
        while let Some(popped) = state.stack.pop() {
            state.on_stack.remove(&popped);
            component.push(popped);
            if popped == module {
                break;
            }
        }
        state.components.push(component);
    }
}
