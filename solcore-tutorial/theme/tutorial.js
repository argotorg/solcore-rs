// The book owns its project files and embeds the shared Playground application.
// A plain mdBook preview has no generated projects or runtime to open.
window.name = "solcore-tutorial";
const printPage = /\/print\.html$/.test(window.location.pathname);
let activeExercise = null;
let exerciseIndex = 0;

for (const link of document.querySelectorAll("a[href]")) {
  const url = new URL(link.href, window.location.href);
  const projectReference = url.searchParams.get("project");
  const projectUrl = projectReference ? new URL(projectReference, url) : url;
  const projectId = projectUrl.pathname.match(/\/projects\/([a-z0-9-]+)\.json$/)?.[1];
  if (!projectId) continue;

  const bundled = Boolean(projectReference)
    && url.origin === window.location.origin
    && url.pathname.endsWith("/playground/index.html")
    && /^#\/project(?:\?|$)/.test(url.hash);
  const exercise = link.closest(".tutorial-exercise");
  if (!bundled) {
    link.removeAttribute("href");
    link.setAttribute("aria-disabled", "true");
    link.title = "Interactive projects are available in the complete tutorial build";
    if (exercise) {
      link.textContent = `${exercise.dataset.title || "Solcore example"} — prose preview`;
      const note = document.createElement("p");
      note.className = "tutorial-preview-note";
      note.textContent = "Build the complete tutorial to edit and run this example. See the tutorial README for setup instructions.";
      exercise.append(note);
    }
    continue;
  }

  // The project query identifies both the fetched source and its saved workspace.
  // Only the iframe adds embed=1; a full tab keeps the complete editor controls.
  url.searchParams.delete("embed");
  link.href = url.href;
  link.target = `solcore-tutorial-project-${projectId}`;
  link.dataset.playgroundLink = "true";
  link.title = "Open this project in a full tab";
  if (!exercise || printPage) continue;

  const title = exercise.dataset.title || "Solcore example";
  exercise.setAttribute("role", "region");
  exercise.setAttribute("aria-label", `Practice: ${title}`);

  const heading = document.createElement("strong");
  heading.className = "tutorial-exercise-title";
  heading.textContent = `Practice: ${title}`;

  const description = document.createElement("p");
  description.className = "tutorial-exercise-description";
  description.textContent = "Edit and run this example here. Your code is saved separately for each project in this browser.";

  const actions = document.createElement("div");
  actions.className = "tutorial-exercise-actions";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.textContent = "Try here";
  toggle.setAttribute("aria-expanded", "false");
  link.textContent = "Open in full tab ↗";
  actions.append(toggle, link);

  const status = document.createElement("p");
  status.className = "tutorial-exercise-status";
  status.setAttribute("aria-live", "polite");
  status.textContent = "The editor loads when you choose Try here.";

  const frameHost = document.createElement("div");
  frameHost.className = "tutorial-exercise-frame";
  frameHost.id = `tutorial-editor-${projectId}-${++exerciseIndex}`;
  frameHost.hidden = true;
  toggle.setAttribute("aria-controls", frameHost.id);
  exercise.append(heading, description, actions, status, frameHost);

  let frame = null;
  const close = () => {
    frame?.remove();
    frame = null;
    frameHost.hidden = true;
    toggle.textContent = "Try here";
    toggle.setAttribute("aria-expanded", "false");
    status.textContent = "Editor closed. Your saved code will be here when you return.";
    if (activeExercise === close) activeExercise = null;
  };

  toggle.addEventListener("click", () => {
    if (frame) {
      close();
      return;
    }
    // One active compiler/editor at a time, including the project solution.
    activeExercise?.();
    activeExercise = close;
    const embeddedUrl = new URL(url);
    embeddedUrl.searchParams.set("embed", "1");
    frame = document.createElement("iframe");
    frame.title = `Edit and run Solcore: ${title}`;
    frame.src = embeddedUrl.href;
    frame.addEventListener("load", () => {
      status.textContent = "Edit the code below, then use Run, tests, or Try calls. Open in full tab for more space.";
    });
    frameHost.append(frame);
    frameHost.hidden = false;
    toggle.textContent = "Close editor";
    toggle.setAttribute("aria-expanded", "true");
    status.textContent = "Loading editor…";
  });

  // Both views share this project's saved source. Release the inline instance
  // when moving to a full tab so two editors do not write over one another.
  link.addEventListener("click", () => {
    if (frame) close();
  });
}
