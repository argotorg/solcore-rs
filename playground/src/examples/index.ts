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
}

export const examples: PlaygroundExample[] = [
  {
    id: "contract-output",
    name: "Contract output",
    description: "A small contract that emits Hull, Yul, Sonatina IR, and ABI JSON.",
    entry: "main.solc",
    files: [
      {
        path: "main.solc",
        content: `import std.{*};
import std.dispatch.{*};

contract Answer {
  public function main() -> uint256 {
    return uint256(42);
  }
}
`,
      },
    ],
  },
  {
    id: "hello",
    name: "Hello",
    description: "A minimal function returning a word literal.",
    entry: "main.solc",
    files: [
      {
        path: "main.solc",
        content: `function main() -> word {
  return 42;
}
`,
      },
    ],
  },
  {
    id: "std-usage",
    name: "Std usage",
    description: "Imports a helper from the embedded standard library.",
    entry: "main.solc",
    files: [
      {
        path: "main.solc",
        content: `import std.{addWord};

function main() -> word {
  return addWord(1, 2);
}
`,
      },
    ],
  },
  {
    id: "multi-file",
    name: "Multi-file",
    description: "Imports a sibling module and calls an exported function.",
    entry: "main.solc",
    files: [
      {
        path: "main.solc",
        content: `import math.{double};

function main() -> word {
  return double(21);
}
`,
      },
      {
        path: "math.solc",
        content: `function double(x: word) -> word {
  let res: word;
  assembly {
    res := add(x, x)
  }
  return res;
}

export { double };
`,
      },
    ],
  },
];

export const defaultExample = examples[0];

export function getExample(id: string): PlaygroundExample {
  return examples.find((example) => example.id === id) ?? defaultExample;
}
