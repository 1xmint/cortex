(() => {
  const canvas = document.getElementById("canvas");
  const ctx = canvas.getContext("2d");
  const LIMIT = 50;

  const state = { tool: "pen", colour: "#000000", size: 8 };
  const undoStack = [];
  const redoStack = [];
  let last = null; // The previous point of the stroke being drawn, if any

  function fillWhite() {
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
  }

  const snapshot = () => ctx.getImageData(0, 0, canvas.width, canvas.height);

  /** Remembers the canvas before a step, so it can be undone. */
  function beginStep() {
    undoStack.push(snapshot());
    if (undoStack.length > LIMIT) undoStack.shift();
    redoStack.length = 0;
  }

  function choose(selector, chosen) {
    for (const el of document.querySelectorAll(selector)) el.classList.toggle("active", el === chosen);
  }

  function setTool(tool) {
    state.tool = tool;
    choose("#pen, #eraser", document.getElementById(tool));
  }

  const ink = () => (state.tool === "eraser" ? "#ffffff" : state.colour);

  function point(event) {
    const rect = canvas.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  function dot(p) {
    ctx.fillStyle = ink();
    ctx.beginPath();
    ctx.arc(p.x, p.y, state.size / 2, 0, Math.PI * 2);
    ctx.fill();
  }

  function segment(from, to) {
    ctx.strokeStyle = ink();
    ctx.lineWidth = state.size;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.beginPath();
    ctx.moveTo(from.x, from.y);
    ctx.lineTo(to.x, to.y);
    ctx.stroke();
  }

  canvas.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    beginStep();
    last = point(event);
    dot(last);
  });

  window.addEventListener("mousemove", (event) => {
    if (!last) return;
    const p = point(event);
    segment(last, p);
    last = p;
  });

  window.addEventListener("mouseup", () => {
    last = null;
  });

  document.getElementById("pen").addEventListener("click", () => setTool("pen"));
  document.getElementById("eraser").addEventListener("click", () => setTool("eraser"));

  for (const button of document.querySelectorAll(".colour")) {
    button.addEventListener("click", () => {
      state.colour = button.dataset.colour;
      choose(".colour", button);
      setTool("pen");
    });
  }

  for (const button of document.querySelectorAll(".size")) {
    button.addEventListener("click", () => {
      state.size = Number(button.dataset.size);
      choose(".size", button);
    });
  }

  document.getElementById("undo").addEventListener("click", () => {
    if (!undoStack.length) return;
    redoStack.push(snapshot());
    ctx.putImageData(undoStack.pop(), 0, 0);
  });

  document.getElementById("redo").addEventListener("click", () => {
    if (!redoStack.length) return;
    undoStack.push(snapshot());
    ctx.putImageData(redoStack.pop(), 0, 0);
  });

  document.getElementById("clear").addEventListener("click", () => {
    if (!window.confirm("Clear the whole drawing?")) return;
    beginStep();
    fillWhite();
  });

  document.getElementById("export").addEventListener("click", () => {
    canvas.toBlob((blob) => {
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = "drawing.png";
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }, "image/png");
  });

  fillWhite();
  setTool("pen");
  choose(".colour", document.querySelector('.colour[data-colour="#000000"]'));
  choose(".size", document.querySelector('.size[data-size="8"]'));
})();
