(() => {
  const PANEL = "http://127.0.0.1:4388";
  const SKIP = "input, select, textarea, label, summary, [data-question]";
  const FIELDS = "input, select, textarea";
  let annotate = true;

  const post = (message) => parent.postMessage(message, PANEL);
  const clip = (text) => text.replace(/\s+/g, " ").trim().slice(0, 120);

  function selectorOf(el) {
    const parts = [];
    for (; el && el.nodeType === 1; el = el.parentElement) {
      if (el.id) {
        parts.unshift("#" + CSS.escape(el.id));
        break;
      }
      if (el === document.body) {
        parts.unshift("body");
        break;
      }
      const same = [...el.parentNode.children].filter((c) => c.tagName === el.tagName);
      parts.unshift(`${el.tagName.toLowerCase()}:nth-of-type(${same.indexOf(el) + 1})`);
    }
    return parts.join(" > ");
  }

  function svgTarget(el) {
    for (; el instanceof SVGElement; el = el.parentElement) {
      if (el.id) return el;
    }
    return null;
  }

  function cellOf(el) {
    const td = el.closest("td, th");
    if (!td) return null;
    const table = td.closest("table");
    const tables = [...document.querySelectorAll("table")];
    const name = table.caption?.textContent.trim() || table.id || `table ${tables.indexOf(table) + 1}`;
    return {
      table: clip(name),
      row: clip(td.parentElement.cells[0].textContent),
      col: clip(table.rows[0].cells[td.cellIndex]?.textContent ?? ""),
    };
  }

  function note(el, selected) {
    const svg = svgTarget(el);
    if (svg) {
      const title = svg.querySelector(":scope > title");
      return { selector: selectorOf(svg), excerpt: clip(title?.textContent ?? ""), cell: null };
    }
    return {
      selector: selectorOf(el),
      excerpt: clip(selected || el.textContent),
      cell: cellOf(el),
    };
  }

  document.addEventListener(
    "click",
    (e) => {
      if (!annotate || e.target.closest(SKIP)) return;
      e.preventDefault();
      e.stopPropagation();
      const selection = getSelection();
      const selected = selection.toString();
      let target = e.target;
      if (selected.trim()) {
        const common = selection.getRangeAt(0).commonAncestorContainer;
        target = common.nodeType === 1 ? common : common.parentElement;
      }
      post({ type: "note", ...note(target, selected) });
      selection.removeAllRanges();
    },
    true,
  );

  function controls(key) {
    return [...document.querySelectorAll(`[data-question="${CSS.escape(key)}"]`)].flatMap((el) =>
      el.matches(FIELDS) ? [el] : [...el.querySelectorAll(FIELDS)],
    );
  }

  function valueOf(els) {
    if (els.every((el) => el.type === "checkbox")) {
      if (els.length === 1) return els[0].checked ? "yes" : "no";
      return els.filter((el) => el.checked).map((el) => el.value).join(",");
    }
    if (els.every((el) => el.type === "radio")) return els.find((el) => el.checked)?.value ?? "";
    return els[0].value;
  }

  function answer(key) {
    const els = controls(key);
    if (els.length) post({ type: "answer", key, value: valueOf(els) });
  }

  for (const type of ["input", "change"]) {
    document.addEventListener(type, (e) => {
      const question = e.target.closest?.("[data-question]");
      if (question) answer(question.dataset.question);
    });
  }

  addEventListener("message", (e) => {
    if (e.source !== parent || e.data?.type !== "mode") return;
    annotate = e.data.mode === "annotate";
    document.documentElement.style.cursor = annotate ? "crosshair" : "";
  });

  document.documentElement.style.cursor = "crosshair";
  post({ type: "ready" });
  for (const key of new Set([...document.querySelectorAll("[data-question]")].map((el) => el.dataset.question))) {
    answer(key);
  }
})();
