# Jupyter server configuration for the mindfork test lab.
# See docs/research/docker-jupyter-env.md §5 (fork F3).

c = get_config()  # noqa: F821 — injected by traitlets

# The file browser is rooted at the home directory rather than at `work/`, so both
# the mounted host folder (`work/`) and the app's data root (`mindfork/data`) are
# visible. Seeing `chats/*.json`, `logs/` and whatever `/export` wrote is half the
# reason to run the app inside JupyterLab at all
# (docs/history/chat-export-file.md).
c.ServerApp.root_dir = "/home/jovyan"
c.ServerApp.ip = "0.0.0.0"
c.ServerApp.open_browser = False

# CPU is off by default in jupyter-resource-usage because polling it costs a
# little; watching a CPU-only llama.cpp is exactly the case where it earns that.
c.ResourceUseDisplay.track_cpu_percent = True
