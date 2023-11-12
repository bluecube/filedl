let images = [];
let currentIndex = null;

let galleryBg = null;
let galleryImgWrap = null;
let galleryImg = null;
let galleryPrefetch = null;
let closeButton = null;
let prevButton = null;
let nextButton = null;
let descriptionBlock = null;
let downloadLink = null;

function galleryInit() {
    document.querySelectorAll(".dir-listing .image").forEach((entry, index) => {
        let mainLink = entry.querySelector(".main-link");
        let download = entry.querySelector("a.download")
        let thumbnail = entry.querySelector("img.thumbnail")
        images.push([mainLink.innerText, download.href, thumbnail.src]);
        mainLink.addEventListener('click', function(event) {
            linkOnclick(event, index);
        });
    });

    galleryBg = document.getElementById("gallery");
    galleryImgWrap = galleryBg.querySelector(".img-wrap");
    galleryImg = galleryImgWrap.querySelector("img");
    closeButton = galleryBg.querySelector("a.close");
    prevButton = galleryBg.querySelector("a.prev");
    nextButton = galleryBg.querySelector("a.next");
    descriptionBlock = galleryBg.querySelector(".description");
    downloadLink = galleryBg.querySelector(".download");

    addEventListener("popstate", popstate);
    galleryImg.onload = imgOnload;
    closeButton.addEventListener("click", closeOnclick);

    setCurrentBasedOnHash();
}

function openGallery() {
    document.body.classList.add("gallery-visible");
    document.addEventListener("keydown", keydown);
}

function closeGallery() {
    document.removeEventListener("keydown", keydown);
    closeGalleryNoHistory();
    history.pushState(null, "", window.location.pathname);
}

function closeGalleryNoHistory() {
    currentIndex = null;
    document.body.classList.remove("gallery-visible");
}

function hashForIndex(index) {
    return "#" + encodeURIComponent(images[index][0]);
}

function setCurrent(index) {
    setCurrentNoHistory(index);
    history.pushState(currentIndex, "", hashForIndex(currentIndex));
}

function setCurrentNoHistory(index) {
    if (currentIndex === null)
        openGallery();

    if (index < 0)
        index = 0;
    else if (index >= images.length)
        index = images.length - 1;

    currentIndex = index;
    galleryImg.src = images[index][1];
    galleryImgWrap.classList.add("loading");
    descriptionBlock.innerText = (index + 1) + "/" + images.length + " " + images[index][0];
    downloadLink.href = images[index][1];

    if (index > 0) {
        prevButton.href = hashForIndex(index - 1);
        prevButton.classList.remove("hidden");
    } else {
        prevButton.classList.add("hidden");
    }

    if (index < (images.length - 1)) {
        nextButton.href = hashForIndex(index + 1);
        nextButton.classList.remove("hidden");
    } else {
        nextButton.classList.add("hidden");
    }
}

function setCurrentBasedOnHash() {
    if (window.location.hash) {
        console.log("Hash = " + window.location.hash);
        let hash = window.location.hash.substring(1);
        let decoded = decodeURIComponent(hash);
        let index = images.findIndex((im) => im[0] == decoded);
        if (index != -1) {
            console.log("Going to index " + index);
            setCurrentNoHistory(index);
        }
    } else {
        closeGalleryNoHistory();
    }
}

function linkOnclick(event, index) {
    event.preventDefault();
    setCurrent(index);
}

function closeOnclick(event) {
    event.preventDefault();
    closeGallery();
}

function popstate(event) {
    if (event.state === null) {
        console.log("Popstate without state");
        setCurrentBasedOnHash();
    } else {
        console.log("Popstate with state " + event.state);
        setCurrentNoHistory(event.state);
    }
}

function keydown(event) {
    if (event.key == "Escape") {
        closeGallery();
    } else if (event.key == "ArrowLeft" && currentIndex > 0) {
        setCurrent(currentIndex - 1);
    } else if ((event.key == "ArrowRight" || event.key == " ") && currentIndex < (images.length - 1)) {
        setCurrent(currentIndex + 1);
    } else {
        return;
    }

    event.preventDefault();
}

function imgOnload() {
    galleryImgWrap.classList.remove("loading");

    if (currentIndex < (images.length - 1)) {
        galleryPrefetch = new Image();
        galleryPrefetch.src = images[currentIndex + 1][1];
    }
}

galleryInit();