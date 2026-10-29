# Codeyard shell integration (bash): started with --rcfile, so a login shell's files are read here first.
[ -f /etc/profile ] && . /etc/profile
for __cy_f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
  if [ -f "$__cy_f" ]; then . "$__cy_f"; break; fi
done
unset __cy_f
if [[ $- == *i* && -z "$__cy_loaded" && "$TERM_PROGRAM" == codeyard ]]; then
  __cy_loaded=1
  # The editor counts a command as finished (133;D) only after it saw it start (133;C).
  __cy_prompt() {
    local ret=$?
    local __cy_pwd=${PWD//%/%25}
    printf '\e]133;D;%s\a' "$ret"
    printf '\e]7;file://%s%s\a' "$HOSTNAME" "$__cy_pwd"
    printf '\e]133;A\a'
    return $ret
  }
  PROMPT_COMMAND="__cy_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  # bash 4.4 and later show PS0 when a command starts (bash 3.2 marks prompts and folders only).
  if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
    PS0='\[\e]133;C\a\]'"$PS0"
  fi
fi
