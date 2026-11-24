local pane = require("@yi/pane")

return {
    view = function(ctx)
        local q = ctx.http.get("http://ci-ubuntu:9100/queue")
        if not q then
            return { pane.stat("Waiting", "-"), pane.stat("Retrying", "-"), pane.stat("Done", "-") }
        end
        local files = {}
        for _, f in q.files do
            local state = if f.state == "retrying" then { text = f.state, tone = "needs_you" } else f.state
            table.insert(files, { name = f.name, state = state, tries = f.tries })
        end
        return {
            pane.stat("Waiting", q.waiting),
            pane.stat("Retrying", q.retrying, { tone = "needs_you" }),
            pane.stat("Done", q.done),
            pane.spark(q.history, { label = "last 10 minutes" }),
            pane.table(files, { columns = { "name", "state", "tries" }, title = "Files", note = "last 10 minutes above" }),
        }
    end,
}
