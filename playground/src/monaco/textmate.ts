import type * as Monaco from "monaco-editor";
import * as onigModule from "vscode-oniguruma";
import * as textmateModule from "vscode-textmate";
import type { IGrammar, IOnigLib, IRawGrammar, StateStack } from "vscode-textmate";
import solcoreGrammarSource from "../../../editors/vscode-solcore/syntaxes/solcore.tmLanguage.json?raw";
import onigurumaWasmUrl from "vscode-oniguruma/release/onig.wasm?url";

export const SOLCORE_SCOPE_NAME = "source.solcore";

type OnigModule = typeof import("vscode-oniguruma");
type TextMateModule = typeof import("vscode-textmate");

const onig = (
  (onigModule as unknown as { default?: OnigModule }).default ?? onigModule
) as OnigModule;
const textmate = (
  (textmateModule as unknown as { default?: TextMateModule }).default ?? textmateModule
) as TextMateModule;
const { createOnigScanner, createOnigString, loadWASM } = onig;
const { INITIAL, parseRawGrammar, Registry } = textmate;

let onigLibPromise: Promise<IOnigLib> | null = null;
let grammarPromise: Promise<IGrammar> | null = null;

class TextMateState implements Monaco.languages.IState {
  constructor(readonly ruleStack: StateStack) {}

  clone(): Monaco.languages.IState {
    return new TextMateState(this.ruleStack.clone());
  }

  equals(other: Monaco.languages.IState): boolean {
    return other instanceof TextMateState && this.ruleStack.equals(other.ruleStack);
  }
}

function lastScope(scopes: string[]): string {
  return scopes[scopes.length - 1] ?? SOLCORE_SCOPE_NAME;
}

async function loadOnigLib(): Promise<IOnigLib> {
  if (!onigLibPromise) {
    onigLibPromise = (async () => {
      const response = await fetch(onigurumaWasmUrl);
      await loadWASM(response);
      return { createOnigScanner, createOnigString };
    })();
  }

  return onigLibPromise;
}

async function loadSolcoreGrammar(): Promise<IGrammar> {
  if (!grammarPromise) {
    grammarPromise = (async () => {
      const rawGrammar = parseRawGrammar(
        solcoreGrammarSource,
        "solcore.tmLanguage.json",
      ) as IRawGrammar;
      const registry = new Registry({
        onigLib: loadOnigLib(),
        async loadGrammar(scopeName) {
          return scopeName === SOLCORE_SCOPE_NAME ? rawGrammar : null;
        },
      });
      const grammar = await registry.loadGrammar(SOLCORE_SCOPE_NAME);

      if (!grammar) {
        throw new Error(`Failed to load TextMate grammar for ${SOLCORE_SCOPE_NAME}`);
      }

      return grammar;
    })();
  }

  return grammarPromise;
}

export async function createSolcoreTextMateTokensProvider(): Promise<
  Monaco.languages.TokensProvider
> {
  const grammar = await loadSolcoreGrammar();

  return {
    getInitialState() {
      return new TextMateState(INITIAL);
    },
    tokenize(line, state) {
      const ruleStack = state instanceof TextMateState ? state.ruleStack : INITIAL;
      const result = grammar.tokenizeLine(line, ruleStack);

      return {
        endState: new TextMateState(result.ruleStack),
        tokens: result.tokens.map((token) => ({
          startIndex: token.startIndex,
          scopes: lastScope(token.scopes),
        })),
      };
    },
  };
}
