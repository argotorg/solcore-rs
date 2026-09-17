import type { KeyboardEvent, ReactNode } from "react";

function navigateTabs(event: KeyboardEvent<HTMLDivElement>): void {
  const tabs = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]:not(:disabled)'));
  const index = tabs.findIndex((tab) => tab === event.target);
  if (index < 0) return;
  let next: number;
  switch (event.key) {
    case "ArrowRight": next = (index + 1) % tabs.length; break;
    case "ArrowLeft": next = (index + tabs.length - 1) % tabs.length; break;
    case "Home": next = 0; break;
    case "End": next = tabs.length - 1; break;
    default: return;
  }
  event.preventDefault();
  tabs[next].focus();
  tabs[next].click();
}

export function TabList({ label, className, children }: {
  label: string;
  className: string;
  children: ReactNode;
}): JSX.Element {
  return <div role="tablist" aria-label={label} className={className} onKeyDown={navigateTabs}>
    {children}
  </div>;
}
