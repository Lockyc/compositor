// compositor shell behaviour: theme toggle, TOC scroll-spy, Pagefind search.
// Progressive enhancement — every feature degrades gracefully if JS/assets are
// absent (the TOC is still a plain list of links, search box just stays empty).
(function () {
  "use strict";

  // --- Theme toggle (Pico reads data-theme on <html>) ---------------------
  var root = document.documentElement;
  var toggle = document.querySelector(".theme-toggle");
  function current() {
    return (
      root.getAttribute("data-theme") ||
      (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light")
    );
  }
  if (toggle) {
    toggle.setAttribute("aria-pressed", current() === "dark" ? "true" : "false");
    toggle.addEventListener("click", function () {
      var next = current() === "dark" ? "light" : "dark";
      root.setAttribute("data-theme", next);
      toggle.setAttribute("aria-pressed", next === "dark" ? "true" : "false");
      try { localStorage.setItem("theme", next); } catch (e) {}
    });
  }

  // --- Mobile nav drawer --------------------------------------------------
  var navToggle = document.querySelector(".nav-toggle");
  var menu = document.getElementById("nav");
  if (navToggle && menu) {
    navToggle.addEventListener("click", function () {
      var open = menu.classList.toggle("is-open-on-mobile");
      navToggle.setAttribute("aria-expanded", open ? "true" : "false");
      root.classList.toggle("nav-open", open);
    });
  }

  // --- TOC scroll-spy -----------------------------------------------------
  var toc = document.getElementById("toc");
  if (toc && "IntersectionObserver" in window) {
    var links = {};
    toc.querySelectorAll("a[href^='#']").forEach(function (a) {
      links[a.getAttribute("href").slice(1)] = a;
    });
    var headings = document.querySelectorAll("#doc h2[id], #doc h3[id]");
    var observer = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (entry) {
          if (!entry.isIntersecting) return;
          var link = links[entry.target.id];
          if (!link) return;
          Object.keys(links).forEach(function (id) {
            links[id].removeAttribute("aria-current");
          });
          link.setAttribute("aria-current", "true");
        });
      },
      { rootMargin: "0px 0px -70% 0px", threshold: 0 }
    );
    headings.forEach(function (h) { observer.observe(h); });
  }

  // --- Pagefind search UI (present only in `build` output) ----------------
  if (window.PagefindUI) {
    try {
      new window.PagefindUI({ element: "#search", showSubResults: true });
    } catch (e) {}
  }
})();
