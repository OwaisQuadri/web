"use strict";

const state = {
  actionCount: 0,
  details: null,
  kind: document.body.dataset.kind,
  ready: false,
};

function publishReady(details) {
  state.details = details;
  state.ready = true;
  document.body.dataset.benchmarkReady = "true";
  document.body.dataset.benchmarkDetails = JSON.stringify(details);
}

function buildArticle() {
  const article = document.querySelector("article");
  for (let index = 1; index <= 100; index += 1) {
    if ((index - 1) % 10 === 0) {
      const heading = document.createElement("h2");
      heading.textContent = `Section ${Math.ceil(index / 10)}`;
      article.append(heading);
    }
    const paragraph = document.createElement("p");
    paragraph.textContent = `Paragraph ${index}. The fixed browser workload repeats stable local text to exercise layout, selection, scrolling, and retained document state.`;
    article.append(paragraph);
  }
  publishReady({ paragraphs: article.querySelectorAll("p").length });
}

async function buildImages() {
  const images = [...document.querySelectorAll("img")];
  await Promise.all(images.map((image) => image.decode()));
  publishReady({
    images: images.length,
    pixels: images.reduce((sum, image) => sum + image.naturalWidth * image.naturalHeight, 0),
  });
}

function buildApplication() {
  const body = document.querySelector("tbody");
  const fragment = document.createDocumentFragment();
  for (let index = 1; index <= 2000; index += 1) {
    const row = document.createElement("tr");
    row.innerHTML = `<td>${index}</td><td>Record ${index}</td><td>${(index * 17) % 1000}</td>`;
    fragment.append(row);
  }
  body.append(fragment);
  document.querySelector("button").addEventListener("click", () => {
    document.querySelector("output").value = document.querySelector("input").value;
  });
  publishReady({ rows: body.rows.length });
}

window.webBenchmarkStatus = () => ({ ...state });
window.webBenchmarkAct = () => {
  if (!state.ready) {
    throw new Error("benchmark fixture is not ready");
  }
  state.actionCount += 1;
  if (state.kind === "application") {
    const input = document.querySelector("input");
    input.value = `updated-${state.actionCount}`;
    document.querySelector("button").click();
    state.details.output = document.querySelector("output").value;
  } else {
    window.scrollTo(0, document.documentElement.scrollHeight);
    window.scrollTo(0, 0);
  }
  return { ...state };
};

if (state.kind === "article") {
  buildArticle();
} else if (state.kind === "images") {
  buildImages().catch((error) => {
    document.body.dataset.benchmarkError = String(error);
  });
} else if (state.kind === "application") {
  buildApplication();
} else {
  document.body.dataset.benchmarkError = "unknown fixture kind";
}
