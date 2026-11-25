//! The stack locations that `--ethdebug` advertises for parameters and locals,
//! checked against the stack of the running code.
//!
//! The fixtures are compiled with the driver at optimization level 0 with debug
//! information, deployed in revm, and called. An inspector records the program
//! counter and the whole stack before every executed opcode. The format says
//! that a debugger paused before step `k` applies the context of the opcode that
//! ran at step `k - 1`, so at every step the stack pointers of that opcode's
//! context are read from the real stack and judged:
//!
//! 1. the slot exists (slot 0 is the top of the stack);
//! 2. a variable's word changes between two observations only if an opcode that
//!    belongs to an assignment to it ran in between (checked for the variables
//!    of functions that run once, so that one activation is meant);
//! 3. where the test knows the value the source requires, the word is that
//!    value. This covers the pointers of the locals that two fixtures declare,
//!    not the pointers of the locals of the standard library, which only have
//!    to exist (1).
//!
//! Properties 1 and 2 are checked on every fixture with a runnable entry point
//! (`every_fixture_...`); property 3 only on the two with named values. A test
//! that passes because no location is advertised anywhere would be worthless,
//! so the named values are asserted to occur. Each test prints a summary (run
//! with `--nocapture`): the executed steps, the pointers evaluated, how many of
//! them were compared with a required value, the problems found, and for each
//! named local how many steps inside its function have a location.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
};

use revm::primitives::U256;
#[path = "support/ethdebug_output.rs"]
mod ethdebug_output;

use ethdebug_output::{CALLER, Step, call_data, compile, execute, repo_root};

/// One variable that an opcode's context lists.
#[derive(Clone)]
struct Listed {
    name: String,
    /// Offset of the declaring identifier, when it is in the fixture's own file:
    /// `None` for a field and for a variable that std declares.
    declaration: Option<u64>,
    /// Whether the variable is a parameter or local of some function.
    local: bool,
    /// The slot of a stack pointer, counted from the top.
    slot: Option<u64>,
}

/// The context of one opcode of the runtime program.
struct Instruction {
    /// Offset and length of the source range the opcode was made from.
    code: Option<(u64, u64)>,
    variables: Vec<Listed>,
}

/// What one call of a contract saw.
struct Run {
    text: String,
    instructions: Rc<HashMap<usize, Instruction>>,
    steps: Vec<Step>,
    /// The word in storage slot 0 after the call.
    stored: U256,
}

/// Compiles `tests/ethdebug/<fixture>/main.sol` with debug information, deploys
/// `contract` and makes each of `calls` (a signature and its arguments; none
/// for a contract without dispatch), one transaction each.
fn run_calls(fixture: &str, contract: &str, calls: &[(&str, &[u64])]) -> Vec<Run> {
    let dir = repo_root().join("tests/ethdebug").join(fixture);
    let text = std::fs::read_to_string(dir.join("main.sol")).unwrap();
    let output = compile(&dir, &["--ethdebug"]);
    let init = output.bytes(&format!("{contract}.bin"));
    let runtime = output.bytes(&format!("{contract}.bin-runtime"));
    let program = output.json(&format!("{contract}_ethdebug-runtime.json"));
    let main_id = output.source_id("main.sol");

    let mut instructions = HashMap::new();
    for instruction in program["instructions"].as_array().unwrap() {
        let context = &instruction["context"];
        let code = &context["code"];
        let code = (code["source"]["id"] == main_id).then(|| {
            (
                code["range"]["offset"].as_u64().unwrap(),
                code["range"]["length"].as_u64().unwrap(),
            )
        });
        let variables = context["variables"]
            .as_array()
            .map(|variables| {
                variables
                    .iter()
                    .map(|variable| Listed {
                        name: variable["identifier"].as_str().unwrap().to_owned(),
                        declaration: (variable["declaration"]["source"]["id"] == main_id)
                            .then(|| variable["declaration"]["range"]["offset"].as_u64())
                            .flatten(),
                        local: variable.get("declaration").is_some(),
                        slot: (variable["pointer"]["location"] == "stack")
                            .then(|| variable["pointer"]["slot"].as_u64().unwrap()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        instructions.insert(
            instruction["offset"].as_u64().unwrap() as usize,
            Instruction { code, variables },
        );
    }
    let instructions = Rc::new(instructions);

    let data: Vec<Vec<u8>> = calls
        .iter()
        .map(|(signature, arguments)| {
            if signature.is_empty() {
                Vec::new()
            } else {
                let words: Vec<U256> = arguments.iter().map(|&word| U256::from(word)).collect();
                call_data(signature, &words)
            }
        })
        .collect();
    let execution = execute(&init, &runtime, &data);
    // The public functions do not return their result to the caller, so the
    // fixtures with named values keep it in the first storage slot.
    let stored = execution
        .storage
        .get(&U256::ZERO)
        .copied()
        .unwrap_or_default();
    execution
        .calls
        .into_iter()
        .map(|steps| Run {
            text: text.clone(),
            instructions: Rc::clone(&instructions),
            steps,
            stored,
        })
        .collect()
}

/// Compiles the fixture, deploys the contract and calls its `main()`.
fn run(fixture: &str, contract: &str) -> Run {
    let [run] = <[Run; 1]>::try_from(run_calls(fixture, contract, &[("main()", &[])]))
        .unwrap_or_else(|_| unreachable!());
    run
}

/// A variable of the fixture, which two declarations of one name tell apart.
type Ident = (String, u64);

/// A stack pointer that a context advertised, read at one step.
struct Observation {
    /// The step before which the context was read.
    step: usize,
    ident: Ident,
    word: U256,
}

/// Everything that the evaluation of one run found.
struct Evaluation {
    observations: Vec<Observation>,
    /// Steps at which a stack pointer was evaluated, and pointers evaluated.
    steps_with_pointers: usize,
    pointers: usize,
    /// For each local, the steps before which its context lists it.
    listed: BTreeMap<Ident, BTreeSet<usize>>,
    problems: Vec<String>,
}

/// Evaluates every stack pointer of the context that holds before each step.
fn evaluate(run: &Run) -> Evaluation {
    let mut evaluation = Evaluation {
        observations: Vec::new(),
        steps_with_pointers: 0,
        pointers: 0,
        listed: BTreeMap::new(),
        problems: Vec::new(),
    };
    for step in 1..run.steps.len() {
        let before = &run.steps[step - 1];
        let Some(instruction) = run.instructions.get(&before.pc) else {
            continue;
        };
        let stack = &run.steps[step].stack;
        let mut evaluated = false;
        for variable in &instruction.variables {
            if !variable.local {
                continue;
            }
            let ident = variable
                .declaration
                .map(|declaration| (variable.name.clone(), declaration));
            if let Some(ident) = &ident {
                evaluation
                    .listed
                    .entry(ident.clone())
                    .or_default()
                    .insert(step);
            }
            let Some(slot) = variable.slot else { continue };
            evaluated = true;
            evaluation.pointers += 1;
            // 1. The slot is on the stack. Slot 0 is the top.
            let Some(at) = stack.len().checked_sub(1 + slot as usize) else {
                evaluation.problems.push(format!(
                    "step {step}: `{}` is at slot {slot}, but the stack has {} words",
                    variable.name,
                    stack.len()
                ));
                continue;
            };
            if let Some(ident) = ident {
                evaluation.observations.push(Observation {
                    step,
                    ident,
                    word: stack[at],
                });
            }
        }
        evaluation.steps_with_pointers += usize::from(evaluated);
    }
    evaluation
}

/// The byte range of `needle` in `text`, which must occur once.
fn span(text: &str, needle: &str) -> (u64, u64) {
    let start = text.find(needle).unwrap_or_else(|| panic!("`{needle}`"));
    assert_eq!(text.matches(needle).count(), 1, "`{needle}` is not unique");
    (start as u64, (start + needle.len()) as u64)
}

/// The text of a function from `header` to the brace that closes it.
fn function_span(text: &str, header: &str) -> (u64, u64) {
    let start = text.find(header).unwrap_or_else(|| panic!("`{header}`"));
    let open = start + text[start..].find('{').unwrap();
    let mut depth = 0;
    for (at, byte) in text[open..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (start as u64, (open + at + 1) as u64);
                }
            }
            _ => {}
        }
    }
    panic!("`{header}` is not closed");
}

fn within(code: Option<(u64, u64)>, (start, end): (u64, u64)) -> bool {
    code.is_some_and(|(offset, length)| start <= offset && offset + length <= end)
}

/// The spans of the statements in `function` that assign to the variable called
/// `name`: `let name ...;` and `name = ...;`, and the update of a `for`.
fn assignments(text: &str, function: (u64, u64), name: &str) -> Vec<(u64, u64)> {
    let body = &text[function.0 as usize..function.1 as usize];
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut found = Vec::new();
    for (at, _) in body.match_indices(name) {
        let before = body[..at].bytes().next_back();
        let after = &body[at + name.len()..];
        if before.is_some_and(is_word) || after.bytes().next().is_some_and(is_word) {
            continue;
        }
        let start = if body[..at].ends_with("let ") {
            at - 4
        } else if after.starts_with(" = ") {
            at
        } else {
            continue;
        };
        // To the `;` or the `)` that closes the header of a `for`.
        let mut depth = 0;
        let mut end = body.len();
        for (offset, byte) in body[at..].bytes().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' if depth == 0 => {
                    end = at + offset;
                    break;
                }
                b')' => depth -= 1,
                b';' if depth == 0 => {
                    end = at + offset + 1;
                    break;
                }
                _ => {}
            }
        }
        found.push((function.0 + start as u64, function.0 + end as u64));
    }
    found
}

/// Property 2: between two observations of a variable the word changes only if
/// an opcode of an assignment to it ran in between.
fn check_assignments(
    run: &Run,
    evaluation: &Evaluation,
    function: (u64, u64),
    problems: &mut Vec<String>,
) -> usize {
    let mut checked = 0;
    let idents: BTreeSet<Ident> = evaluation
        .observations
        .iter()
        .filter(|observation| (function.0..function.1).contains(&observation.ident.1))
        .map(|observation| observation.ident.clone())
        .collect();
    for ident in idents {
        let spans = assignments(&run.text, function, &ident.0);
        // The steps whose opcode belongs to an assignment to the variable.
        let assigning: Vec<usize> = (0..run.steps.len())
            .filter(|&step| {
                run.instructions
                    .get(&run.steps[step].pc)
                    .is_some_and(|instruction| {
                        spans.iter().any(|&span| within(instruction.code, span))
                    })
            })
            .collect();
        let mut previous: Option<&Observation> = None;
        for observation in evaluation.observations.iter().filter(|o| o.ident == ident) {
            if let Some(before) = previous.filter(|before| before.word != observation.word) {
                // The context read before step `n` is that of the opcode of
                // step `n - 1`: the opcodes in between are those of the steps
                // `before.step ..= observation.step - 1`.
                let ran = assigning
                    .iter()
                    .any(|&step| before.step <= step && step < observation.step);
                if !ran {
                    problems.push(format!(
                        "`{}`: the word changed from {} to {} between steps {} and {} without an assignment",
                        ident.0, before.word, observation.word, before.step, observation.step
                    ));
                }
            }
            checked += usize::from(previous.is_some());
            previous = Some(observation);
        }
    }
    checked
}

/// What the checks of one run found: the problems, and the observations (by
/// index) whose word was compared with a value that the source requires.
struct Judge {
    problems: Vec<String>,
    compared: BTreeSet<usize>,
}

impl Judge {
    fn new(evaluation: &Evaluation) -> Self {
        Judge {
            problems: evaluation.problems.clone(),
            compared: BTreeSet::new(),
        }
    }

    /// Judges the observations of `ident` at steps whose preceding opcode is
    /// inside `statement`: every word must be one of `allowed`, and every word
    /// of `required` must occur. Without `required` the variable need not be
    /// located at all.
    fn expect(
        &mut self,
        what: &str,
        (run, evaluation): (&Run, &Evaluation),
        ident: &Ident,
        statement: (u64, u64),
        allowed: &BTreeSet<U256>,
        required: &BTreeSet<U256>,
    ) {
        let indices = observed(run, evaluation, ident, statement);
        let found: BTreeSet<U256> = indices
            .iter()
            .map(|&at| evaluation.observations[at].word)
            .collect();
        self.compared.extend(indices);
        if !found.is_subset(allowed) {
            self.problems.push(format!(
                "{what}: words {found:?} are not within {allowed:?}"
            ));
        }
        for value in required {
            if !found.contains(value) {
                self.problems
                    .push(format!("{what}: the word {value} was never located"));
            }
        }
    }

    /// Judges one observation against the words it may hold.
    fn expect_one(&mut self, what: &str, at: usize, word: U256, allowed: &[u64]) {
        self.compared.insert(at);
        if !allowed.iter().any(|&value| U256::from(value) == word) {
            self.problems
                .push(format!("{what}: holds {word}, expected one of {allowed:?}"));
        }
    }
}

/// The observations (indices into `evaluation.observations`) of `ident` at
/// steps whose preceding opcode is inside `statement`.
fn observed(
    run: &Run,
    evaluation: &Evaluation,
    ident: &Ident,
    statement: (u64, u64),
) -> Vec<usize> {
    evaluation
        .observations
        .iter()
        .enumerate()
        .filter(|(_, observation)| {
            observation.ident == *ident
                && within(
                    run.instructions
                        .get(&run.steps[observation.step - 1].pc)
                        .and_then(|instruction| instruction.code),
                    statement,
                )
        })
        .map(|(at, _)| at)
        .collect()
}

fn set(values: &[u64]) -> BTreeSet<U256> {
    values.iter().map(|&value| U256::from(value)).collect()
}

/// The local called `name` that is declared inside `function`.
fn local(evaluation: &Evaluation, name: &str, function: (u64, u64)) -> Ident {
    let found: Vec<&Ident> = evaluation
        .listed
        .keys()
        .filter(|(listed, at)| listed == name && (function.0..function.1).contains(at))
        .collect();
    assert_eq!(found.len(), 1, "`{name}` in {function:?}: {found:?}");
    found[0].clone()
}

/// Steps before which a context of the function's own statements was read, and
/// how many of them list and locate `ident`.
fn share(
    run: &Run,
    evaluation: &Evaluation,
    ident: &Ident,
    function: (u64, u64),
) -> (usize, usize, usize) {
    let inside: BTreeSet<usize> = (1..run.steps.len())
        .filter(|&step| {
            within(
                run.instructions
                    .get(&run.steps[step - 1].pc)
                    .and_then(|instruction| instruction.code),
                function,
            )
        })
        .collect();
    let listed = evaluation.listed.get(ident).map_or(0, |steps| {
        steps.iter().filter(|step| inside.contains(step)).count()
    });
    let located = evaluation
        .observations
        .iter()
        .filter(|o| o.ident == *ident && inside.contains(&o.step))
        .count();
    (inside.len(), listed, located)
}

/// The summary of one run, printed with `--nocapture`: the executed steps, the
/// pointers evaluated, how many of them are of locals that the fixture's own
/// file declares and how many of those had their word compared with a value
/// that the source requires, the problems, and for each named local the steps
/// inside its function and the steps with a location.
fn report(
    label: &str,
    run: &Run,
    evaluation: &Evaluation,
    judge: &Judge,
    shares: &[(&str, Ident, (u64, u64))],
) {
    println!(
        "{label}: {} executed steps, {} steps with a stack pointer, {} pointers evaluated, {} of them of locals of the fixture, {} of those compared with a required value, {} problems",
        run.steps.len(),
        evaluation.steps_with_pointers,
        evaluation.pointers,
        evaluation.observations.len(),
        judge.compared.len(),
        judge.problems.len()
    );
    for (name, ident, function) in shares {
        let (inside, listed, located) = share(run, evaluation, ident, *function);
        println!(
            "{label}: `{name}`: {located} of {inside} steps inside its function have a location ({:.1}%), {located} of {listed} steps that list it ({:.1}%)",
            100.0 * located as f64 / inside.max(1) as f64,
            100.0 * located as f64 / listed.max(1) as f64
        );
    }
}

fn finish(judge: &Judge) {
    assert!(
        judge.problems.is_empty(),
        "{} problems:\n{}",
        judge.problems.len(),
        judge.problems.join("\n")
    );
}

#[test]
fn stack_locations_of_the_storage_fields_fixture_match_the_running_code() {
    let run = run("storage-fields", "StorageFields");
    // `count = f(uint256(0))`, and `f` returns the second `total`.
    assert_eq!(run.stored, U256::from(17));
    let evaluation = evaluate(&run);
    let text = &run.text;
    let main = function_span(text, "function main()");
    let ident = |needle: &str, shift: usize| -> Ident {
        let at = text.find(needle).unwrap_or_else(|| panic!("`{needle}`")) + shift;
        ("total".to_owned(), at as u64)
    };
    let first = ident("let total: uint256 = count", 4);
    let second = ident("let total: uint256 = total + uint256(1);", 4);
    let first_let = span(text, "let total: uint256 = count + uint256(10);");
    let reassignment = span(text, "total = total + bonus;");
    let second_let = span(text, "let total: uint256 = total + uint256(1);");
    let final_return = span(text, "return count;");
    let everywhere = (0, text.len() as u64);
    let context = (&run, &evaluation);

    // 3. The values the source requires: with `count` 1, the first `total` is 11,
    //    then 16 after the reassignment in the taken branch and at the join, and
    //    the second is 17.
    let mut judge = Judge::new(&evaluation);
    judge.expect(
        "first `total` after its `let`",
        context,
        &first,
        first_let,
        &set(&[11]),
        &set(&[11]),
    );
    judge.expect(
        "first `total` at the reassignment",
        context,
        &first,
        reassignment,
        &set(&[11, 16]),
        &set(&[16]),
    );
    judge.expect(
        "first `total` at the join",
        context,
        &first,
        second_let,
        &set(&[16]),
        &set(&[16]),
    );
    judge.expect(
        "second `total` after its `let`",
        context,
        &second,
        second_let,
        &set(&[17]),
        &set(&[17]),
    );
    judge.expect(
        "first `total` everywhere",
        context,
        &first,
        everywhere,
        &set(&[11, 16]),
        &set(&[11, 16]),
    );
    judge.expect(
        "second `total` everywhere",
        context,
        &second,
        everywhere,
        &set(&[17]),
        &set(&[17]),
    );
    // The second `total` was consumed by the call that returns `count`: its
    // value is not on the stack at the final return, though another word that
    // equals 17 is. No pointer for either `total` may be advertised there.
    for ident in [&first, &second] {
        let found = observed(&run, &evaluation, ident, final_return);
        if !found.is_empty() {
            judge.problems.push(format!(
                "`total` has a location at the final return: {} pointers",
                found.len()
            ));
        }
    }

    // The binders of a pattern, the parameter of a lambda and a local that
    // assembly assigns. `extent` runs once, on `Shape.Seg(1, 17)`; the lambda
    // runs once, on 0; `caller` runs once, and its result is the caller of the
    // transaction, which no other word of the run equals.
    let extent = function_span(text, "function extent(");
    let lambda = function_span(text, "lam (v");
    let caller = function_span(text, "function caller()");
    let lo = local(&evaluation, "lo", extent);
    let hi = local(&evaluation, "hi", extent);
    let v = local(&evaluation, "v", lambda);
    let res = local(&evaluation, "res", caller);
    let caller_word = U256::from_be_slice(CALLER.as_slice());
    let only = |word: U256| BTreeSet::from([word]);
    judge.expect("`lo`", context, &lo, everywhere, &set(&[1]), &set(&[1]));
    judge.expect("`hi`", context, &hi, everywhere, &set(&[17]), &set(&[17]));
    judge.expect("`v`", context, &v, everywhere, &set(&[0]), &set(&[0]));
    let assigned = span(text, "res := caller()");
    judge.expect(
        "`res` once assigned",
        context,
        &res,
        assigned,
        &only(caller_word),
        &only(caller_word),
    );

    // 2. A word changes only at an assignment.
    let checked = check_assignments(&run, &evaluation, main, &mut judge.problems);
    report(
        "storage-fields",
        &run,
        &evaluation,
        &judge,
        &[
            ("total (first)", first, main),
            ("total (second)", second, main),
            ("lo", lo, extent),
            ("hi", hi, extent),
            ("v", v, lambda),
            ("res", res, caller),
        ],
    );
    println!("storage-fields: {checked} pairs of observations compared");
    assert!(checked > 0);
    finish(&judge);
}

/// The iteration of `acc = acc + a;` that each step belongs to: `None` before
/// the statement was first entered, otherwise the number of entries so far, minus
/// one. The statement is entered at its first opcode.
fn iterations(run: &Run, update: (u64, u64)) -> Vec<Option<usize>> {
    let entry = run
        .instructions
        .iter()
        .filter(|(_, instruction)| within(instruction.code, update))
        .map(|(&pc, _)| pc)
        .min()
        .expect("the update has code");
    let mut entered = 0usize;
    let iteration: Vec<Option<usize>> = run
        .steps
        .iter()
        .map(|step| {
            if step.pc == entry {
                entered += 1;
            }
            entered.checked_sub(1)
        })
        .collect();
    assert_eq!(entered, 4, "the loop runs four times");
    iteration
}

#[test]
fn stack_locations_of_parameters_and_a_loop_accumulator_match_the_running_code() {
    let run = run("stack-values", "StackValues");
    // mix(3, 4) is four additions of 3, and keep(5) is (5 + 100) * 1000 + 5.
    assert_eq!(run.stored, U256::from(12 + 105_005));
    let evaluation = evaluate(&run);
    let text = &run.text;
    let mix = function_span(text, "function mix(");
    let keep = function_span(text, "function keep(");
    let ident = |name: &str, needle: &str, shift: usize| -> Ident {
        let at = text.find(needle).unwrap() + shift;
        assert_eq!(&text[at..at + name.len()], name);
        (name.to_owned(), at as u64)
    };
    let a = ident("a", "(a: uint256", 1);
    let b = ident("b", "b: uint256)", 0);
    let acc = ident("acc", "let acc", 4);
    let i = ident("i", "let i: uint256", 4);
    let update = span(text, "acc = acc + a;");
    let counter_update = span(text, "i = i + uint256(1)");
    let context = (&run, &evaluation);
    let everywhere = (0, text.len() as u64);

    let mut judge = Judge::new(&evaluation);
    // The parameters never change: every location holds the argument, and each
    // is located.
    for (name, ident, argument) in [("a", &a, 3u64), ("b", &b, 4)] {
        judge.expect(
            &format!("parameter `{name}`"),
            context,
            ident,
            everywhere,
            &set(&[argument]),
            &set(&[argument]),
        );
    }

    // In iteration `n` of the loop (counted from the first opcode of
    // `acc = acc + a;` to the one before the next entry), `acc` is 3n inside
    // that statement before it completes and 3(n + 1) once it has, and `i` is
    // `n` inside it and from the update on `n + 1`. Before the first iteration
    // both are 0. The update `i = i + 1` is the one statement in which `i` goes
    // from `n` to `n + 1`.
    let iteration = iterations(&run, update);
    let mut located_in_iteration = [0usize; 4];
    let mut steps_in_iteration = [0usize; 4];
    for step in 1..run.steps.len() {
        let inside = within(
            run.instructions
                .get(&run.steps[step - 1].pc)
                .and_then(|instruction| instruction.code),
            update,
        );
        if inside && let Some(n) = iteration[step - 1] {
            steps_in_iteration[n] += 1;
        }
    }
    for (at, observation) in evaluation.observations.iter().enumerate() {
        if observation.ident != acc && observation.ident != i {
            continue;
        }
        let code = run
            .instructions
            .get(&run.steps[observation.step - 1].pc)
            .and_then(|instruction| instruction.code);
        let n = iteration[observation.step - 1];
        let (inside_update, inside_counter) = (within(code, update), within(code, counter_update));
        let (what, allowed): (String, Vec<u64>) = if observation.ident == acc {
            let allowed = match n {
                Some(n) if inside_update => vec![3 * n as u64, 3 * (n as u64 + 1)],
                Some(n) => vec![3 * (n as u64 + 1)],
                None => vec![0],
            };
            if inside_update && let Some(n) = n {
                located_in_iteration[n] += 1;
            }
            (
                format!("`acc` at step {} (iteration {n:?})", observation.step),
                allowed,
            )
        } else {
            let allowed = match n {
                Some(n) if inside_update => vec![n as u64],
                Some(n) if inside_counter => vec![n as u64, n as u64 + 1],
                Some(n) => vec![n as u64 + 1],
                None => vec![0],
            };
            (
                format!("`i` at step {} (iteration {n:?})", observation.step),
                allowed,
            )
        };
        judge.expect_one(&what, at, observation.word, &allowed);
    }
    println!(
        "stack-values: `acc` in `acc = acc + a;` has a location at {located_in_iteration:?} of {steps_in_iteration:?} steps per iteration"
    );
    if located_in_iteration.contains(&0) {
        judge
            .problems
            .push("an iteration of the loop has no location for `acc`".to_owned());
    }
    // The counter is located in every iteration, not only in the body.
    for n in 0..4 {
        let located = evaluation
            .observations
            .iter()
            .filter(|o| o.ident == i && iteration[o.step - 1] == Some(n))
            .count();
        if located == 0 {
            judge
                .problems
                .push(format!("`i` has no location in iteration {n}"));
        }
    }

    // The share of the steps inside the whole loop (header, body and update)
    // at which the accumulator has a location.
    let the_loop = function_span(text, "for (let i");
    let (in_loop, _, located_in_loop) = share(&run, &evaluation, &acc, the_loop);
    println!(
        "stack-values: `acc` has a location at {located_in_loop} of {in_loop} steps inside the loop"
    );
    if located_in_loop == 0 {
        judge
            .problems
            .push("`acc` has no location inside the loop".to_owned());
    }

    // A value that stays on the stack after the variable that held it was
    // assigned again. `y` holds the old value of `x` for good, so the word 5
    // stays on the stack with the labels of both `(y, 5)` and `(x, 5)`. The
    // pointers of `x` are advertised only once `x = x + 100;` has run, so
    // wherever `x` is located its word is the new value 105 and never the old
    // word 5, and the return statement, which reads `x`, has such a pointer.
    let p = local(&evaluation, "p", keep);
    let x = local(&evaluation, "x", keep);
    let y = local(&evaluation, "y", keep);
    let keep_return = span(text, "return x * uint256(1000) + y;");
    judge.expect(
        "parameter `p`",
        context,
        &p,
        everywhere,
        &set(&[5]),
        &set(&[5]),
    );
    judge.expect("`y`", context, &y, everywhere, &set(&[5]), &set(&[5]));
    judge.expect("`x`", context, &x, everywhere, &set(&[105]), &set(&[105]));
    judge.expect(
        "`x` at the return",
        context,
        &x,
        keep_return,
        &set(&[105]),
        &set(&[105]),
    );

    let mut checked = check_assignments(&run, &evaluation, mix, &mut judge.problems);
    checked += check_assignments(&run, &evaluation, keep, &mut judge.problems);
    report(
        "stack-values",
        &run,
        &evaluation,
        &judge,
        &[
            ("a", a, mix),
            ("b", b, mix),
            ("acc", acc, mix),
            ("i", i, mix),
            ("p", p, keep),
            ("x", x, keep),
            ("y", y, keep),
        ],
    );
    println!("stack-values: {checked} pairs of observations compared");
    assert!(checked > 0);
    finish(&judge);
}

/// A contract of a fixture, the calls that run it, and the functions of it that
/// run once in each call.
struct Entry {
    fixture: &'static str,
    contract: &'static str,
    /// Signature and arguments of each call; an empty signature is a call with no
    /// data, for a contract that does not dispatch.
    calls: &'static [(&'static str, &'static [u64])],
    /// The functions (a text that precedes them, and their header) whose
    /// variables property 2 is checked for. Each runs at most once per call.
    once: &'static [(Option<&'static str>, &'static str)],
    /// The fewest stack pointers that the calls must evaluate, so that a fixture
    /// whose locals all lost their pointers fails.
    pointers: usize,
}

const MAIN: &[(&str, &[u64])] = &[("main()", &[])];

const ENTRIES: &[Entry] = &[
    Entry {
        fixture: "storage-fields",
        contract: "StorageFields",
        calls: MAIN,
        once: &[(None, "function main(")],
        pointers: 1,
    },
    Entry {
        fixture: "stack-values",
        contract: "StackValues",
        calls: MAIN,
        once: &[(None, "function mix("), (None, "function keep(")],
        pointers: 1,
    },
    Entry {
        fixture: "scopes",
        contract: "Scopes",
        calls: &[
            ("loop_sum()", &[]),
            ("block_sum(uint256)", &[1]),
            ("captured()", &[]),
        ],
        once: &[
            (None, "function loop_sum("),
            (None, "function block_sum("),
            (None, "function captured("),
        ],
        pointers: 1,
    },
    Entry {
        fixture: "scopes",
        contract: "Hiding",
        calls: MAIN,
        once: &[(Some("contract Hiding"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "no-std",
        contract: "AfterUnit",
        calls: &[("", &[])],
        once: &[(Some("contract AfterUnit"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "no-std",
        contract: "Tagged",
        calls: &[("", &[])],
        once: &[(Some("contract Tagged"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "storage-slots",
        contract: "Slots",
        calls: MAIN,
        once: &[(Some("contract Slots"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "storage-slots",
        contract: "AfterBool",
        calls: MAIN,
        once: &[(Some("contract AfterBool"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "types",
        contract: "Types",
        calls: MAIN,
        once: &[(Some("contract Types"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "types",
        contract: "Early",
        calls: MAIN,
        once: &[(Some("contract Early"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "types",
        contract: "AfterBool",
        calls: MAIN,
        once: &[(Some("contract AfterBool"), "function main(")],
        pointers: 0,
    },
    Entry {
        fixture: "types",
        contract: "AfterUnit",
        calls: MAIN,
        once: &[(Some("contract AfterUnit"), "function main(")],
        pointers: 0,
    },
];

/// Properties 1 and 2 on every contract of every fixture that has an entry
/// point: each advertised stack slot exists on the stack of the running code, and
/// a variable's advertised word changes only at an assignment to it.
#[test]
fn every_fixture_with_an_entry_point_has_stack_locations_that_match_the_running_code() {
    let fixtures: BTreeSet<String> = ethdebug_output::fixtures()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let covered: BTreeSet<String> = ENTRIES
        .iter()
        .map(|entry| entry.fixture.to_owned())
        .collect();
    assert_eq!(fixtures, covered, "every fixture needs an entry");

    for entry in ENTRIES {
        let label = format!("{} {}", entry.fixture, entry.contract);
        let (mut steps, mut pointers, mut observations, mut checked) = (0, 0, 0, 0);
        let mut problems = Vec::new();
        for run in run_calls(entry.fixture, entry.contract, entry.calls) {
            let evaluation = evaluate(&run);
            problems.extend(evaluation.problems.iter().cloned());
            for (marker, header) in entry.once {
                let base = marker.map_or(0, |marker| run.text.find(marker).unwrap());
                let (start, end) = function_span(&run.text[base..], header);
                let function = (start + base as u64, end + base as u64);
                checked += check_assignments(&run, &evaluation, function, &mut problems);
            }
            steps += run.steps.len();
            pointers += evaluation.pointers;
            observations += evaluation.observations.len();
        }
        println!(
            "{label}: {steps} executed steps, {pointers} stack pointers evaluated, {observations} of them of locals of the fixture, {checked} pairs of observations compared, {} problems",
            problems.len()
        );
        assert!(
            pointers >= entry.pointers,
            "{label}: {pointers} pointers evaluated"
        );
        assert!(problems.is_empty(), "{label}: {}", problems.join("\n"));
    }
}
