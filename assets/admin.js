async function deleteObject(button) {
    const listItem = button.closest('li');
    const id = button.value;
    const resp = await fetch('objects/' + encodeURIComponent(id), { method: 'DELETE' });
    if (resp.ok) listItem.remove();
    else alert('Delete failed: ' + resp.status);
}

const form = document.getElementById('create-form');
const fileInput = form.querySelector('[name=file]');
const pathInput = form.querySelector('[name=path]');
const idInput = form.querySelector('[name=object-id]');
const overrideCheckbox = form.querySelector('[name=override-id]');

function derivedId() {
    const mode = form.querySelector('[name=mode]:checked').value;
    if (mode === 'upload') {
        return fileInput.files[0] ? fileInput.files[0].name : '';
    } else {
        const parts = pathInput.value.split('/').filter(p => p.length > 0);
        return parts.length > 0 ? parts[parts.length - 1] : '';
    }
}

function syncId() {
    if (!overrideCheckbox.checked) {
        idInput.value = derivedId();
    }
}

overrideCheckbox.addEventListener('change', () => {
    idInput.readOnly = !overrideCheckbox.checked;
    if (!overrideCheckbox.checked) idInput.value = derivedId();
});

function setMode(mode) {
    const uploadMode = mode === 'upload';
    fileInput.classList.toggle('inactive', !uploadMode);
    pathInput.classList.toggle('inactive', uploadMode);
    syncId();
}

form.querySelectorAll('[name=mode]').forEach(radio => {
    radio.addEventListener('change', () => setMode(radio.value));
});

fileInput.addEventListener('click', () => {
    form.querySelector('[name=mode][value=upload]').checked = true;
    setMode('upload');
});

pathInput.addEventListener('focus', () => {
    form.querySelector('[name=mode][value=link]').checked = true;
    setMode('link');
});

fileInput.addEventListener('change', syncId);
pathInput.addEventListener('input', syncId);

form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const id = idInput.value;
    const mode = form.querySelector('[name=mode]:checked').value;
    const unlisted = form.querySelector('[name=unlisted]').checked;
    const result = form.querySelector('.result');

    if (mode === 'upload' && !fileInput.files[0]) {
        result.textContent = 'Please select a file.';
        return;
    }
    if (mode === 'link' && !pathInput.value) {
        result.textContent = 'Please enter a path.';
        return;
    }

    let url = 'objects/' + encodeURIComponent(id);
    let fetchOptions;
    if (mode === 'upload') {
        if (unlisted) url += '?unlisted=true';
        fetchOptions = { method: 'PUT', body: fileInput.files[0] };
    } else {
        url += '?link=' + encodeURIComponent(pathInput.value);
        if (unlisted) url += '&unlisted=true';
        fetchOptions = { method: 'PUT' };
    }

    const resp = await fetch(url, fetchOptions);
    if (resp.ok) {
        location.reload();
    } else {
        const body = await resp.text();
        result.textContent = (mode === 'upload' ? 'Upload' : 'Link') + ' failed: ' + body;
    }
});
