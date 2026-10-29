# Using the editor

Compilation and execution happen in your browser; your source is not sent to a compilation service.

## Open an example

Click **Try here** to open a lesson's editor, or **Open in full tab** for more space.
The first run can take longer while the compiler loads.
**Close editor** closes it while keeping your saved code.

## Run code

- **Compile** checks the program and produces output without executing a function.
- In **Run**, select **Try calls**, choose a function, enter any arguments, and click **Call function**.
- In **Run**, select **Run tests** and click **Run all tests from start** to check the example's test comments.

Manual calls keep their state between calls; tests use a separate contract instance.
A passed test does not prepare the state for manual calls.
See [Public functions and tests](tests.md) and [Contracts and persistent state](state.md) for details.

## Save and reset

Each project keeps your edited source in this browser, including when you return from another lesson.
Inline and full-tab editors share that source; keep only one editor open for a project at a time.

**Reset project** restores the original source.
**Reset contract** clears manual contract state while keeping your code.
Editing source, closing the editor, or reloading the page also starts a new execution session.

Back up important code to a local file before clearing browser data or using a private window.
A project URL shares the original example, not your edits, arguments, or contract state.

## Troubleshooting

- **Editor will not load:** try **Open in full tab** and reload; if that fails, report the page URL to the tutorial host.
- **Function missing from Try calls:** check that it is `public` and compilation succeeded.
- **Unexpected result:** rerun after editing and repeat any manual setup calls; see [Contracts and persistent state](state.md) for how tests and calls differ.
