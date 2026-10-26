# Codeyard shell integration (zsh): this folder is ZDOTDIR only long enough to load the user's files, then the hooks.
__cy_dir="$ZDOTDIR"
ZDOTDIR="${CODEYARD_USER_ZDOTDIR:-$HOME}"
[ -f "$ZDOTDIR/.zshenv" ] && . "$ZDOTDIR/.zshenv"
__cy_user="$ZDOTDIR"
ZDOTDIR="$__cy_dir"
