use super::*;

pub(super) fn best_name_suggestion(
    name: &str,
    candidates: impl IntoIterator<Item = String>,
) -> Option<String> {
    let mut candidates = candidates
        .into_iter()
        .filter(|candidate| candidate != name)
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();

    let mut best: Option<(usize, String)> = None;
    for candidate in candidates {
        let distance = edit_distance(name, &candidate);
        let limit = suggestion_distance_limit(name);
        if distance == 0 || distance > limit {
            continue;
        }
        match &best {
            Some((best_distance, best_candidate))
                if distance > *best_distance
                    || (distance == *best_distance && candidate >= *best_candidate) => {}
            _ => best = Some((distance, candidate)),
        }
    }
    best.map(|(_, candidate)| candidate)
}

// Allows one edit for every 3 characters of the looked-up name, and at least one.
fn suggestion_distance_limit(name: &str) -> usize {
    name.chars().count().max(3) / 3
}

// Counts an insertion, a deletion, a substitution, or a swap of two
// neighbouring characters as one edit each, so that `onwer` is one edit from
// `owner`.
fn edit_distance(left: &str, right: &str) -> usize {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    // Distances from the prefix of `left` two characters back, one character
    // back, and the current one, to every prefix of `right`.
    let mut before_previous = vec![0; right_chars.len() + 1];
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left_chars.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution = usize::from(left_char != right_char);
            let mut distance = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution);
            if left_index > 0
                && right_index > 0
                && *left_char == right_chars[right_index - 1]
                && left_chars[left_index - 1] == *right_char
            {
                distance = distance.min(before_previous[right_index - 1] + 1);
            }
            current[right_index + 1] = distance;
        }
        std::mem::swap(&mut before_previous, &mut previous);
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

pub(super) fn builtin_type_or_class<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "word" => BuiltinKind::Type(BuiltinType::Word),
        "bool" => BuiltinKind::Type(BuiltinType::Bool),
        "()" => BuiltinKind::Type(BuiltinType::Unit),
        "pair" => BuiltinKind::Type(BuiltinType::Pair),
        "sum" => BuiltinKind::Type(BuiltinType::Sum),
        "integer" => BuiltinKind::Type(BuiltinType::Integer),
        "invokable" => BuiltinKind::Class(BuiltinClass::Invokable),
        "Int" => BuiltinKind::Class(BuiltinClass::Int),
        "Str" => BuiltinKind::Class(BuiltinClass::Str),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}

pub(super) fn builtin_term<'db>(name: &str) -> Option<Resolution<'db>> {
    let kind = match name {
        "true" => BuiltinKind::Constructor(BuiltinCtor::True),
        "false" => BuiltinKind::Constructor(BuiltinCtor::False),
        "()" => BuiltinKind::Constructor(BuiltinCtor::Unit),
        "pair" => BuiltinKind::Constructor(BuiltinCtor::Pair),
        "inl" => BuiltinKind::Constructor(BuiltinCtor::Inl),
        "inr" => BuiltinKind::Constructor(BuiltinCtor::Inr),
        "invoke" => BuiltinKind::Function(BuiltinFunction::Invoke),
        "primAddWord" => BuiltinKind::Function(BuiltinFunction::PrimAddWord),
        "primEqWord" => BuiltinKind::Function(BuiltinFunction::PrimEqWord),
        "wordToInteger" => BuiltinKind::Function(BuiltinFunction::WordToInteger),
        "wordFromInteger" => BuiltinKind::Function(BuiltinFunction::WordFromInteger),
        "integerAdd" => BuiltinKind::Function(BuiltinFunction::IntegerAdd),
        "integerSub" => BuiltinKind::Function(BuiltinFunction::IntegerSub),
        "integerMul" => BuiltinKind::Function(BuiltinFunction::IntegerMul),
        "integerLt" => BuiltinKind::Function(BuiltinFunction::IntegerLt),
        "integerEq" => BuiltinKind::Function(BuiltinFunction::IntegerEq),
        "invokable.invoke" => BuiltinKind::ClassMethod(BuiltinClassMethod::InvokableInvoke),
        "Int.fromInteger" => BuiltinKind::ClassMethod(BuiltinClassMethod::IntFromInteger),
        "Str.fromString" => BuiltinKind::ClassMethod(BuiltinClassMethod::StrFromString),
        _ => return None,
    };
    Some(Resolution::Builtin(kind))
}
