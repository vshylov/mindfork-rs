# A short orientation banner in the JupyterLab terminal. Login shells only, and
# only interactive ones — jupyter_server_terminals starts bash with `-l`, and
# anything printed unconditionally here would also land in the output of every
# non-interactive `docker compose exec … bash -lc …`.
#
# TERM is deliberately left exactly as the terminal set it: reproducing what a
# real JupyterLab terminal does is the whole point of this environment, and
# "fixing" it here would hide the difference this container exists to expose.

case $- in
*i*) ;;
*) return ;;
esac

printf '\n  mindfork test lab\n'
printf '    mindfork            start the app (chat: %s, embeddings: %s)\n' \
    "${LAB_CHAT_URL:-http://chat:8000/v1}" \
    "${LAB_EMBED_URL:-http://embed:8001/v1}"
printf '    mindfork --help     commands: backup, restore, import, demo, sandbox setup\n'
printf '    data                ~/mindfork/data  (settings.json, chats/, logs/, data.db)\n'
printf '    files               ~/work           (mounted from the host)\n\n'
