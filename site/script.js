// Copy buttons on install command blocks: copy the command, not the shell prompt.

function copyViaSelection(text) {
  const scratch = document.createElement('textarea');
  scratch.value = text;
  scratch.setAttribute('readonly', '');
  scratch.style.position = 'fixed';
  scratch.style.opacity = '0';
  document.body.appendChild(scratch);
  scratch.select();
  const ok = document.execCommand('copy');
  scratch.remove();
  return ok ? Promise.resolve() : Promise.reject(new Error('copy refused'));
}

function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) {
    // Headless or restricted browsers can reject the async API; fall back.
    return navigator.clipboard.writeText(text).catch(() => copyViaSelection(text));
  }
  return copyViaSelection(text);
}

document.querySelectorAll('.code-copy').forEach((button) => {
  button.addEventListener('click', () => {
    const command = button.closest('.code-block')?.querySelector('.cmd-text');
    if (!command) return;
    copyText(command.textContent.trim()).then(
      () => {
        button.textContent = 'copied';
        button.classList.add('copied');
      },
      () => {
        button.textContent = 'failed';
      },
    ).finally(() => {
      setTimeout(() => {
        button.textContent = 'copy';
        button.classList.remove('copied');
      }, 1600);
    });
  });
});
