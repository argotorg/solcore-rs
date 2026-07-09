import { useEffect, useState } from "react";
import { nowMs } from "../compiler/timing";
import { useWorkspaceStore } from "../store/workspace";

export function useCompileElapsed(): number | null {
  const compiling = useWorkspaceStore((state) => state.compiling);
  const compileStartedAt = useWorkspaceStore((state) => state.compileStartedAt);
  const [currentTime, setCurrentTime] = useState(() => nowMs());

  useEffect(() => {
    if (!compiling || compileStartedAt === null) {
      return;
    }

    setCurrentTime(nowMs());
    const timer = window.setInterval(() => setCurrentTime(nowMs()), 100);
    return () => window.clearInterval(timer);
  }, [compileStartedAt, compiling]);

  if (!compiling || compileStartedAt === null) {
    return null;
  }

  return Math.max(0, currentTime - compileStartedAt);
}
