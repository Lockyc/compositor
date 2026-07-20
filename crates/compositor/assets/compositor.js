// compositor shell behaviour: theme toggle, TOC scroll-spy.
// Progressive enhancement — every feature degrades gracefully if JS/assets are
// absent (the TOC is still a plain list of links).
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

  // --- Center the active nav item on load ---------------------------------
  // The active path is expanded server-side, so a deep current page can land
  // below the sidebar's own scroll fold. Center it within #nav's scroll box
  // (never the window). scrollTop is clamped, so a shallow/top item stays put
  // (delta <= 0 -> 0) and only a genuinely deep item scrolls. Progressive
  // enhancement: no active item -> no-op.
  if (menu) {
    var active = menu.querySelector('a[aria-current="page"]');
    if (active) {
      var navRect = menu.getBoundingClientRect();
      var aRect = active.getBoundingClientRect();
      var delta = aRect.top - navRect.top - menu.clientHeight / 2 + aRect.height / 2;
      menu.scrollTop += delta;
    }
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
})();
