ZDOTDIR="$__cy_user"
# The system's zshrc ran while ZDOTDIR was this folder and chose the history file in it: back to the user's own (a HISTFILE the user's files set still wins).
[[ "$HISTFILE" == "$__cy_dir/.zsh_history" ]] && HISTFILE="$ZDOTDIR/.zsh_history"
[ -f "$ZDOTDIR/.zshrc" ] && . "$ZDOTDIR/.zshrc"
__cy_user="$ZDOTDIR"
ZDOTDIR="$__cy_dir"
if [[ -o interactive && -z "$__cy_loaded" && "$TERM_PROGRAM" == codeyard ]]; then
  __cy_loaded=1
  __cy_ran=
  __cy_precmd() {
    local ret=$?
    local __cy_pwd=${PWD//\%/%25}
    [[ -n "$__cy_ran" ]] && printf '\e]133;D;%s\a' "$ret"
    __cy_ran=
    printf '\e]7;file://%s%s\a' "${HOST}" "$__cy_pwd"
    printf '\e]133;A\a'
    return $ret
  }
  __cy_preexec() {
    __cy_ran=1
    printf '\e]133;C\a'
  }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __cy_precmd
  add-zsh-hook preexec __cy_preexec
fi
