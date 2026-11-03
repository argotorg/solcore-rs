import contractOutputHello from "./contract-output/Hello.sol?raw";
import stdUsageCalculator from "./std-usage/Calculator.sol?raw";
import traitLightSwitch from "./trait/LightSwitch.sol?raw";
import genericsRegistry from "./generics/Registry.sol?raw";
import optionSplitter from "./option/Splitter.sol?raw";
import optionOption from "./option/option.sol?raw";
import patternMatchingEscrow from "./pattern-matching/Escrow.sol?raw";
import miniNftMiniNFT from "./mini-nft/MiniNFT.sol?raw";
import miniNftOption from "./mini-nft/option.sol?raw";
import compositionVaults from "./composition/Vaults.sol?raw";
import compositionEngine from "./composition/engine.sol?raw";
import invariantsAmm from "./invariants/Amm.sol?raw";
import invariantsPool from "./invariants/pool.sol?raw";
import extensionsToken from "./extensions/Token.sol?raw";
import extensionsErc20core from "./extensions/erc20core.sol?raw";
import extensionsPausable from "./extensions/pausable.sol?raw";
import extensionsCapped from "./extensions/capped.sol?raw";
import extensionsOwnable from "./extensions/ownable.sol?raw";
import reentrancyVault from "./reentrancy/Vault.sol?raw";
import reentrancyReentrancy from "./reentrancy/reentrancy.sol?raw";
import comptimeAnswer from "./comptime/Answer.sol?raw";
import workshop1Token from "./workshop-1/Token.sol?raw";
import workshop1DoneToken from "./workshop-1-done/Token.sol?raw";
import workshop2Token from "./workshop-2/Token.sol?raw";
import workshop2DoneToken from "./workshop-2-done/Token.sol?raw";
import workshop3Token from "./workshop-3/Token.sol?raw";
import workshop3Ownable from "./workshop-3/ownable.sol?raw";
import workshop3DoneToken from "./workshop-3-done/Token.sol?raw";
import workshop3DoneOwnable from "./workshop-3-done/ownable.sol?raw";
import workshop4Token from "./workshop-4/Token.sol?raw";
import workshop4Events from "./workshop-4/events.sol?raw";
import workshop4Ownable from "./workshop-4/ownable.sol?raw";
import workshopSolutionToken from "./workshop-solution/Token.sol?raw";
import workshopSolutionEvents from "./workshop-solution/events.sol?raw";
import workshopSolutionOwnable from "./workshop-solution/ownable.sol?raw";
import workshopDemoHooksToken from "./workshop-demo-hooks/Token.sol?raw";
import workshopDemoHooksChange from "./workshop-demo-hooks/change.sol?raw";
import workshopDemoHooksHooks from "./workshop-demo-hooks/hooks.sol?raw";
import workshopDemoHooksEvents from "./workshop-demo-hooks/events.sol?raw";
import workshopDemoHooksOwnable from "./workshop-demo-hooks/ownable.sol?raw";
import workshopBonusToken from "./workshop-bonus/Token.sol?raw";
import workshopBonusChange from "./workshop-bonus/change.sol?raw";
import workshopBonusHooks from "./workshop-bonus/hooks.sol?raw";
import workshopBonusEvents from "./workshop-bonus/events.sol?raw";
import workshopBonusOwnable from "./workshop-bonus/ownable.sol?raw";
import workshopBonusDoneToken from "./workshop-bonus-done/Token.sol?raw";
import workshopBonusDoneChange from "./workshop-bonus-done/change.sol?raw";
import workshopBonusDoneHooks from "./workshop-bonus-done/hooks.sol?raw";
import workshopBonusDoneEvents from "./workshop-bonus-done/events.sol?raw";
import workshopBonusDoneOwnable from "./workshop-bonus-done/ownable.sol?raw";

export interface ExampleFile {
  path: string;
  content: string;
}

export interface PlaygroundExample {
  id: string;
  name: string;
  description: string;
  entry: string;
  files: ExampleFile[];
  /** An example with a group is listed only while an example of the same group is open. */
  group?: string;
}

export const examples: PlaygroundExample[] = [
  {
    id: "contract-output",
    name: "Hello contract",
    description: "A small contract that emits Hull, Yul, and ABI JSON.",
    entry: "Hello.sol",
    files: [
      { path: "Hello.sol", content: contractOutputHello },
    ],
  },
  {
    id: "std-usage",
    name: "Std usage",
    description: "Calls a standard-library trait method directly; the + operator desugars to the same call.",
    entry: "Calculator.sol",
    files: [
      { path: "Calculator.sol", content: stdUsageCalculator },
    ],
  },
  {
    id: "trait",
    name: "Trait",
    description: "A trait implemented for a custom enum drives a stored on-chain state machine.",
    entry: "LightSwitch.sol",
    files: [
      { path: "LightSwitch.sol", content: traitLightSwitch },
    ],
  },
  {
    id: "generics",
    name: "Generics",
    description: "A where-constrained generic max works for a user-defined Version type via its Ord impl.",
    entry: "Registry.sol",
    files: [
      { path: "Registry.sol", content: genericsRegistry },
    ],
  },
  {
    id: "option",
    name: "Option",
    description: "A reusable Option module and a splitter contract: checked division represents a zero divisor with None.",
    entry: "Splitter.sol",
    files: [
      { path: "Splitter.sol", content: optionSplitter },
      { path: "option.sol", content: optionOption },
    ],
  },
  {
    id: "pattern-matching",
    name: "Pattern matching",
    description: "An escrow whose lifecycle is an enum stored in a contract field, driven by match.",
    entry: "Escrow.sol",
    files: [
      { path: "Escrow.sol", content: patternMatchingEscrow },
    ],
  },
  {
    id: "mini-nft",
    name: "Mini NFT",
    description: "An NFT with typed ownership: tokens either have an owner or do not exist, with no zero-address sentinels.",
    entry: "MiniNFT.sol",
    files: [
      { path: "MiniNFT.sol", content: miniNftMiniNFT },
      { path: "option.sol", content: miniNftOption },
    ],
  },
  {
    id: "composition",
    name: "Composition",
    description: "Three deployable vaults share one engine module: traits compose where Classic Solidity builds inheritance diamonds.",
    entry: "Vaults.sol",
    files: [
      { path: "Vaults.sol", content: compositionVaults },
      { path: "engine.sol", content: compositionEngine },
    ],
  },
  {
    id: "invariants",
    name: "Invariants",
    description: "A constant-product pool as an opaque type: it can only be created or changed through module operations that preserve the invariant.",
    entry: "Amm.sol",
    files: [
      { path: "Amm.sol", content: invariantsAmm },
      { path: "pool.sol", content: invariantsPool },
    ],
  },
  {
    id: "extensions",
    name: "Extensions",
    description: "A token built from owner, pause and cap modules: each module exports a state struct that the contract stores as a field, and transfer, mint and burn all call one update function.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: extensionsToken },
      { path: "erc20core.sol", content: extensionsErc20core },
      { path: "pausable.sol", content: extensionsPausable },
      { path: "capped.sol", content: extensionsCapped },
      { path: "ownable.sol", content: extensionsOwnable },
    ],
  },
  {
    id: "reentrancy",
    name: "Reentrancy",
    description: "Read and write locks as wrapper functions: protected bodies are lambdas, and reads are guarded against read-only reentrancy.",
    entry: "Vault.sol",
    files: [
      { path: "Vault.sol", content: reentrancyVault },
      { path: "reentrancy.sol", content: reentrancyReentrancy },
    ],
  },
  {
    id: "comptime",
    name: "Comptime",
    description: "Evaluates a typed computation during specialization and embeds its result.",
    entry: "Answer.sol",
    files: [
      { path: "Answer.sol", content: comptimeAnswer },
    ],
  },
  {
    id: "workshop-1",
    group: "workshop",
    name: "Workshop 1: transfer",
    description: "Step 1 of the workshop: implement transfer on an ERC-20-style token with a balances mapping until the inline tests pass.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop1Token },
    ],
  },
  {
    id: "workshop-1-done",
    group: "workshop",
    name: "Workshop 1: transfer (solution)",
    description: "Solution to step 1 of the workshop.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop1DoneToken },
    ],
  },
  {
    id: "workshop-2",
    group: "workshop",
    name: "Workshop 2: one Change type",
    description: "Step 2 of the workshop: every balance change is a value of one Change enum, and the starter compiles only once match covers every case.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop2Token },
    ],
  },
  {
    id: "workshop-2-done",
    group: "workshop",
    name: "Workshop 2: one Change type (solution)",
    description: "Solution to step 2 of the workshop.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop2DoneToken },
    ],
  },
  {
    id: "workshop-3",
    group: "workshop",
    name: "Workshop 3: importing onlyOwner",
    description: "Step 3 of the workshop: guard mint and hand over ownership with functions imported from an ownable module that hides its constructor.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop3Token },
      { path: "ownable.sol", content: workshop3Ownable },
    ],
  },
  {
    id: "workshop-3-done",
    group: "workshop",
    name: "Workshop 3: importing onlyOwner (solution)",
    description: "Solution to step 3 of the workshop.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop3DoneToken },
      { path: "ownable.sol", content: workshop3DoneOwnable },
    ],
  },
  {
    id: "workshop-4",
    group: "workshop",
    name: "Workshop 4: comptime",
    description: "Step 4 of the workshop: compute the Transfer event topic at compile time through a comptime parameter.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshop4Token },
      { path: "events.sol", content: workshop4Events },
      { path: "ownable.sol", content: workshop4Ownable },
    ],
  },
  {
    id: "workshop-solution",
    group: "workshop",
    name: "Workshop: final solution",
    description: "The finished workshop token: one Change type, an imported ownable module and a Transfer topic computed at compile time.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshopSolutionToken },
      { path: "events.sol", content: workshopSolutionEvents },
      { path: "ownable.sol", content: workshopSolutionOwnable },
    ],
  },
  {
    id: "workshop-demo-hooks",
    group: "workshop",
    name: "Workshop demo: hooks as values",
    description: "The workshop token with transfer hooks as values: one trait, one impl per hook, and Stacked combines them without override(A, B).",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshopDemoHooksToken },
      { path: "change.sol", content: workshopDemoHooksChange },
      { path: "hooks.sol", content: workshopDemoHooksHooks },
      { path: "events.sol", content: workshopDemoHooksEvents },
      { path: "ownable.sol", content: workshopDemoHooksOwnable },
    ],
  },
  {
    id: "workshop-bonus",
    group: "workshop",
    name: "Workshop bonus: write a hook",
    description: "Bonus for the workshop: write a NoBurn hook as a trait impl and stack three hooks with the one generic impl for Stacked.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshopBonusToken },
      { path: "change.sol", content: workshopBonusChange },
      { path: "hooks.sol", content: workshopBonusHooks },
      { path: "events.sol", content: workshopBonusEvents },
      { path: "ownable.sol", content: workshopBonusOwnable },
    ],
  },
  {
    id: "workshop-bonus-done",
    group: "workshop",
    name: "Workshop bonus: write a hook (solution)",
    description: "Solution to the workshop bonus.",
    entry: "Token.sol",
    files: [
      { path: "Token.sol", content: workshopBonusDoneToken },
      { path: "change.sol", content: workshopBonusDoneChange },
      { path: "hooks.sol", content: workshopBonusDoneHooks },
      { path: "events.sol", content: workshopBonusDoneEvents },
      { path: "ownable.sol", content: workshopBonusDoneOwnable },
    ],
  },
];

export const defaultExample = examples[0];

export function findExample(id: string): PlaygroundExample | undefined {
  return examples.find((example) => example.id === id);
}

/** The examples to list while `id` is open: those in its group, or those without a group. */
export function listedExamples(id: string): PlaygroundExample[] {
  const group = findExample(id)?.group;
  return examples.filter((example) => example.group === group);
}

export function getExample(id: string): PlaygroundExample {
  return findExample(id) ?? defaultExample;
}
