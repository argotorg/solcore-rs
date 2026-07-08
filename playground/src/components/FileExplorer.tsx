import { FileCode2, Pencil, Plus, Trash2 } from "lucide-react";
import { useWorkspaceStore } from "../store/workspace";

export function FileExplorer(): JSX.Element {
  const order = useWorkspaceStore((state) => state.order);
  const entry = useWorkspaceStore((state) => state.entry);
  const activePath = useWorkspaceStore((state) => state.activePath);
  const createFile = useWorkspaceStore((state) => state.createFile);
  const renameFile = useWorkspaceStore((state) => state.renameFile);
  const deleteFile = useWorkspaceStore((state) => state.deleteFile);
  const setActive = useWorkspaceStore((state) => state.setActive);
  const setEntry = useWorkspaceStore((state) => state.setEntry);

  const handleAdd = (): void => {
    const path = window.prompt("New file path", "untitled.solc");
    if (path) {
      createFile(path);
    }
  };

  const handleRename = (path: string): void => {
    const nextPath = window.prompt("Rename file", path);
    if (nextPath) {
      renameFile(path, nextPath);
    }
  };

  const handleDelete = (path: string): void => {
    if (order.length <= 1) {
      return;
    }

    if (window.confirm(`Delete ${path}?`)) {
      deleteFile(path);
    }
  };

  return (
    <aside className="file-explorer" aria-label="Workspace files">
      <div className="panel-heading">
        <div>
          <span className="panel-heading__eyebrow">Workspace</span>
          <h2>Files</h2>
        </div>
        <button type="button" className="icon-button icon-button--small" onClick={handleAdd}>
          <Plus size={16} />
          <span className="sr-only">Add file</span>
        </button>
      </div>

      <div className="file-list">
        {order.map((path) => {
          const isEntry = path === entry;
          const isActive = path === activePath;

          return (
            <div key={path} className={`file-row ${isActive ? "is-active" : ""}`}>
              <button
                type="button"
                className="file-row__main"
                onClick={() => setActive(path)}
                onDoubleClick={() => handleRename(path)}
                title={path}
              >
                <FileCode2 size={16} aria-hidden="true" />
                <span className="file-row__name">{path}</span>
                {isEntry ? <span className="entry-dot" title="Entry file" /> : null}
              </button>

              <div className="file-row__actions">
                {!isEntry ? (
                  <button
                    type="button"
                    className="icon-button icon-button--tiny"
                    onClick={() => setEntry(path)}
                    title="Set as entry"
                  >
                    <span className="entry-target" aria-hidden="true" />
                    <span className="sr-only">Set as entry</span>
                  </button>
                ) : null}
                <button
                  type="button"
                  className="icon-button icon-button--tiny"
                  onClick={() => handleRename(path)}
                  title="Rename"
                >
                  <Pencil size={13} />
                  <span className="sr-only">Rename</span>
                </button>
                <button
                  type="button"
                  className="icon-button icon-button--tiny"
                  onClick={() => handleDelete(path)}
                  disabled={order.length <= 1}
                  title="Delete"
                >
                  <Trash2 size={13} />
                  <span className="sr-only">Delete</span>
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </aside>
  );
}
