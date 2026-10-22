// Basic variables
const scrollbuttonid = "scrollbutton";
const navbuttonid = "navigation-button";
const navid = "navigation";
const listid = "list";
const listselectid = "listselect";
const inputselector = "input[name='keyword']";

const hiddenclass = "hidden-important";
const showifsearchingclasses = ["showifsearching"];
const hideifsearchingclasses = [
  "hideifsearching",
  "ueberschrift-gruppe",
  "category",
];
const categoryclass = "category";
const searchableclasses = ["searchable", "handschrift", "handschrift-liste"];
const hideprevsiblingbtnclass = "hideprevbutton";

// Elements
let listelement = document.getElementById(listid);
let navelelement = document.getElementById(navid);
let navbtnelement = document.getElementById(navbuttonid);
let scrollbtnelement = document.getElementById(scrollbuttonid);
let listselectelement = document.getElementById(listselectid);
let inputelement = document.querySelector(inputselector);
let categoryelements = document.getElementsByClassName(categoryclass);
let hideprevsiblingbtnelements = document.getElementsByClassName(hideprevsiblingbtnclass);
let showifsearchingelements = document.querySelectorAll("." + showifsearchingclasses.join(", ."));
let hideifsearchingelements = document.querySelectorAll("." + hideifsearchingclasses.join(", ."));
let searchableelements = document.querySelectorAll("." + searchableclasses.join(", ."));

// State
let dictionary = [];
let found = dictionary;
let sw = "";
let swl = 0;
let term = "";

const hideifinarr = function (arr) {
  for (var el of arr) {
      el.classList.add(hiddenclass);
  }
};

const showifinarr = function (arr) {
  for (var el of arr) {
    el.classList.remove(hiddenclass);
  }
};

// Script for showing and acting upon the "scroll to top button"
const scrollFunction = function () {
  if (scrollbtnelement !== null) {
    if (
      document.body.scrollTop > 400 ||
      document.documentElement.scrollTop > 400
    ) {
      scrollbtnelement.style.pointerEvents = "auto";
      scrollbtnelement.classList.remove(hiddenclass);
    } else {
      scrollbtnelement.style.pointerEvents = "none";
      scrollbtnelement.classList.add(hiddenclass);
    }
  }
};

const isInViewport = function (element) {
  const rect = element.getBoundingClientRect();
  return (
    rect.top >= 0 &&
    rect.left >= 0 &&
    rect.bottom <=
      (window.innerHeight || document.documentElement.clientHeight) &&
    rect.right <= (window.innerWidth || document.documentElement.clientWidth)
  );
}

const createIndex = function(id) {
  for (var s of searchableelements) {
      dictionary.push({
        category: s.closest("." + categoryclass),
        element: s,
        searchitem: $(s).text().toLowerCase(),
      });
  }
}

const findWord = function (word, d) {
  var sw = word.trim().toLowerCase();
  return d.filter(function (e) {
    if (e.searchitem.indexOf(sw) !== -1) {
      return true;
    } else {
      return false;
    }
  });
}

const search = function (evt) {
  evt = evt || window.event;
  if (evt !== undefined) {
    var isEscape = false;
    if ("key" in evt) {
        isEscape = evt.key === "Escape" || evt.key === "Esc";
    } else {
        isEscape = evt.keyCode === 27;
    }
    if (isEscape) {
        searchreset();
        return;
    }
}

  term = $(inputselector).val().trim() || "";

  // Unmark everything previously marked
  for (let item of found) {
    $(item.element).unmark();
  }

  if (term) {
    // Hide everything tagged hideifseaching
    showifinarr(showifsearchingelements);
    hideifinarr(hideifsearchingelements);

    // Only search in prior found elements if word starts the same
    if (term.length > swl && term.startsWith(sw)) {
      found = findWord(term, found);
    } else {
      found = findWord(term, dictionary);
    }

    for (let item of dictionary) {
      if (found.indexOf(item) !== -1) {
        item.category.classList.add("search-expanded");
        item.category.classList.remove(hiddenclass);
        let prevbutton = item.category.getElementsByClassName(hideprevsiblingbtnclass);
        if (prevbutton.length > 0) {
          prevbutton[0].previousElementSibling.classList.remove(hiddenclass);
        } 
        item.element.classList.remove(hiddenclass);
        if (term.length >= 2) {
          $(item.element).mark(term, {
            separateWordSearch: false,
            acrossElements: true,
          });
        }
      } else {
        item.element.classList.add(hiddenclass);
      }
    }
    sw = term;
    swl = term.length;
  } else {
    searchreset();
  }
};

const searchreset = function () {
  for (let item of found) {
    $(item.element).unmark();
  }

  var hidden = listelement.querySelectorAll("." + hiddenclass);
  for (let item of hidden) {
    item.classList.remove(hiddenclass);
  }
  found = dictionary;
  sw = "";
  swl = 0;
  showifinarr(hideifsearchingelements);
  hideifinarr(showifsearchingelements);
  inputelement.value = "";
  resethidecat();
};

const resethidecat = function() {
  for (let el of hideprevsiblingbtnelements) {
    let prevsib = el.previousElementSibling;
    if (!prevsib.classList.contains(hiddenclass)) {
      prevsib.classList.add(hiddenclass);
    }
  }
}

const toggleghidecat = function(cat) {
  let prevsib = cat.previousElementSibling;
  if (prevsib.classList.contains(hiddenclass)) {
    prevsib.classList.remove(hiddenclass);
  } else {
    prevsib.classList.add(hiddenclass);
  }
}

const initHoverSwap = function () {
  const imgs = document.querySelectorAll("img[data-hover]");
  for (let img of imgs) {
    if (!img.dataset.src) {
      img.dataset.src = img.getAttribute("src");
    }
    const showHover = function () {
      img.setAttribute("src", img.dataset.hover);
    };
    const showOriginal = function () {
      img.setAttribute("src", img.dataset.src);
    };
    img.addEventListener("mouseenter", showHover);
    img.addEventListener("mouseleave", showOriginal);
    img.addEventListener("touchstart", showHover, { passive: true });
    img.addEventListener("touchend", showOriginal);
  }
};


// Mobile menu
if (navbtnelement !== null && navelelement !== null) {
  navbtnelement.addEventListener("click", () => {
    if (navelelement.classList.contains(hiddenclass)) {
      navelelement.classList.remove(hiddenclass);
    } else {
      navelelement.classList.add(hiddenclass);
    }
  });
}

document.addEventListener("DOMContentLoaded", () => {
  initHoverSwap();
});

// Hide category button
for (let el of hideprevsiblingbtnelements) { 
  el.addEventListener("click", () => {
    toggleghidecat(el);
  });
}

// Scroll button
if (scrollbtnelement !== null) {
  scrollFunction();
  scrollbtnelement.addEventListener("click", () => {
    document.body.scrollTop = 0; // For Safari
    document.documentElement.scrollTop = 0; // For Chrome, Firefox, IE and Opera
  });
  window.addEventListener("scroll", scrollFunction);
}

// List filtering
if (listelement) {
  var searchable = "";
  createIndex(listid);
  if (inputelement !== null && isInViewport(inputelement)) {
    inputelement.focus();
  }
  $(inputelement).keyup(search);
  search(null);

  // Scrolling & List updating
  if (listselectelement !== null) {
    var sections = {};
    var i = 0;
    Array.prototype.forEach.call(categoryelements, function (e) {
      sections[e.id] = e.offsetTop;
    });
    window.onscroll = function () {
      var scrollPosition =
        document.documentElement.scrollTop || document.body.scrollTop;
      for (i in sections) {
        if (sections[i] <= scrollPosition + 60) {
          var opt = listselectelement.querySelector('option[value="#' + i + '"]');
          if (opt !== null) {
            opt.selected = true;
          }
        }
      }
    };
  }
}

hideifinarr(showifsearchingelements);
