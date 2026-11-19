local pane = require("@yi/pane")

return {
    view = function(ctx)
        local q = ctx.http.get("http://ci-ubuntu:9100/queue")
        return { pane.stat("Waiting", q.waiting), pane.stat("Lost", q.missing.count) }
    end,
}
