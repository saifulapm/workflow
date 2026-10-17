// Run by `playwright-cli run-code` from t149-parity.sh, which replaces the
// placeholder in `const job` with {"pages": [{"name", "old", "new"}], "out": dir}. Each
// page is opened from both sides at 390 px, the differences decided on purpose
// are blanked the same way on both, and the viewport is saved as
// <out>/<name>.old.png and <out>/<name>.new.png.
async (page) => {
  const job = JOB;
  await page.setViewportSize({ width: 390, height: 844 });
  for (const entry of job.pages) {
    for (const side of ["old", "new"]) {
      await page.goto(entry[side]);
      await page.evaluate(() => {
        const set = (selector, text) => {
          for (const el of document.querySelectorAll(selector)) el.textContent = text;
        };
        // The machine name in the header: the hub's host against next-hub.
        set("header.top h1", "machine");
        // The route header.
        for (const el of document.querySelectorAll("p.route, form.route, form[action='/route']")) {
          el.style.visibility = "hidden";
        }
        // Home's sibling hubs and subscribe link.
        for (const el of document.querySelectorAll("nav.pages")) {
          if (el.querySelector("a[href='/subscribe']")) el.textContent = "nav";
        }
        // Relative ages, which move with the clock.
        const age = /\b\d+(s|m|h|d)\b|\bnow\b/g;
        for (const el of document.querySelectorAll(".meta")) {
          for (const node of el.childNodes) {
            if (node.nodeType === 3) node.textContent = node.textContent.replace(age, "AGE");
          }
        }
      });
      await page.screenshot({ path: `${job.out}/${entry.name}.${side}.png` });
    }
  }
  return "shot";
}
