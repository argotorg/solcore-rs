import { GripVertical } from "lucide-react";
import { PanelResizeHandle } from "react-resizable-panels";

interface ResizeHandleProps {
  direction?: "horizontal" | "vertical";
}

export function ResizeHandle({ direction = "horizontal" }: ResizeHandleProps): JSX.Element {
  return (
    <PanelResizeHandle
      className={`resize-handle resize-handle--${direction}`}
      aria-label="Resize panes"
    >
      <GripVertical aria-hidden="true" size={14} />
    </PanelResizeHandle>
  );
}
