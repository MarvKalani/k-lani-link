// k-lani-link landing page — progressive enhancement only, no network access.
(function () {
  "use strict";

  // Stamp the generation year in the footer.
  var year = document.getElementById("year");
  if (year) {
    year.textContent = String(new Date().getFullYear());
  }

  // Highlight the nav link for the section nearest the viewport centre.
  var links = Array.prototype.slice.call(document.querySelectorAll(".nav-links a"));
  if (!links.length) { return; }

  function setActive() {
    var mid = window.innerHeight / 2;
    var best = null;
    var bestDist = Infinity;
    for (var i = 0; i < links.length; i++) {
      var target = document.getElementById(links[i].getAttribute("href").slice(1));
      if (!target) { continue; }
      var box = target.getBoundingClientRect();
      var dist = Math.abs(box.top + box.height / 2 - mid);
      if (dist < bestDist) {
        bestDist = dist;
        best = links[i];
      }
    }
    for (var j = 0; j < links.length; j++) {
      links[j].removeAttribute("aria-current");
    }
    if (best) { best.setAttribute("aria-current", "page"); }
  }

  var raf = null;
  window.addEventListener("scroll", function () {
    if (raf === null) {
      raf = window.requestAnimationFrame(function () {
        raf = null;
        setActive();
      });
    }
  }, { passive: true });

  setActive();
})();
