"""What to pull from which frame of the built mockup page: panes, data tables and DOM scrapes."""

FRAMES = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 17, 19, 20, 22, 23, 24]


def pane(label, nth=None, within=None):
    s = {"pane": label}
    if nth is not None:
        s["nth"] = nth
    if within:
        s["within"] = within
    return s


def css(*sels):
    return [{"css": s} for s in sels]


# slug, frame, root selectors, row selector (None: no list), extra query
PANES = [
    ("shell", 14, css(".top", ".tree", ".sbar"), None, ""),
    ("shell-folded", 1, css(".top", ".strip", ".sbar"), ".strip .sn", ""),
    ("chat-doc", 17, css(".cfw"), None, "pose=hover"),
    ("new-chat", 19, css(".cfw"), None, ""),
    ("notifications", 4, css(".npanel", ".bell"), ".nt", ""),
    ("menu-bar", 5, css(".mbar"), ".smenu .mi", ""),
    ("context-menu", 14, css(".cmenu"), ".cmi", ""),
    ("context-tray", 23, css(".ctray"), None, ""),
    ("design", 24, [pane("Design")], ".dsr", ""),
    ("agents", 6, [pane("Agents")], ".agr:not(.hd)", ""),
    ("trace", 6, [pane("Trace")], ".spn.xr", ""),
    ("logs", 6, [pane("Logs")], ".lgl", ""),
    ("findings-table", 6, [pane("Findings")], ".fd.xr", ""),
    ("assets", 7, [pane("Assets")], ".ast", ""),
    ("containers", 7, [pane("Containers")], ".ctr", ""),
    ("vms", 7, [pane("VMs")], ".vmr", ""),
    ("daemons", 7, [pane("Daemons")], ".dmn", ""),
    ("debugger", 8, [pane("Debugger")], ".vrr", ""),
    ("references", 8, [pane("rate_limit.rs")], ".rfr", ""),
    ("lsp", 9, [pane("Language servers")], ".lsv", ""),
    ("problems", 9, [pane("Language servers", within=".lgs")], ".dgn", ""),
    ("lsp-code", 9, [pane("rate_limit.rs")], ".dl", ""),
    ("plugins", 9, [pane("Plugins")], ".plg", ""),
    ("findings", 10, [pane("Findings"), pane("F12")], ".f2", ""),
    ("findings-gate", 10, [pane("Ready?")], ".chk", ""),
    ("findings-place", 10, [pane("By place")], ".bar2", ""),
    ("findings-checks", 10, [pane("Standing checks")], ".rul2", ""),
    ("settings-groups", 11, [pane("Settings")], ".fx", ""),
    ("settings", 11, [pane("Appearance")], ".stg", ""),
    ("settings-text", 11, [pane("As text")], None, ""),
    ("themes", 12, [pane("Themes")], ".thm", ""),
    ("theme-tokens", 12, [pane("Edit theme")], ".tk", ""),
    ("plugin-kit", 13, [pane("Chat"), pane("pane.py"), pane("Queue depth"), pane("Plugin kit")], None, ""),
    ("files", 20, [pane("Files", nth=0)], ".fx", ""),
    ("chat", 14, [pane("Chat")], ".at", ""),
    ("changes", 14, [pane("Changes")], ".dl", ""),
    ("code", 1, [pane("rate_limit.rs")], ".dl", ""),
    ("terminal", 14, [pane("zsh")], None, ""),
    ("web", 1, [pane("localhost:5173")], None, ""),
    ("desktop", 1, [pane("ci-ubuntu")], None, ""),
    ("report", 1, [pane("Report")], None, ""),
]

# Where the scale lives: name, selector, dimensions parity compares, whether it is measured with rows open.
PANE_DEFS = [
    ("pane-tab-bar", ".ptabs", ["height"], 0),
    ("pane-tab", ".ptabs .pt", ["height"], 0),
    ("pane-header", ".phd", ["height"], 0),
    ("footer", ".pft", ["height"], 0),
    ("table-header", ".hd", ["height"], 0),
    ("code-line", ".dl", ["height"], 0),
]
ROW_DEFS = [("row", None, ["height"], 0), ("row-detail", ".xd, .lgx", [], 1)]

# Window pieces are measured by these alone, never by the pane scale.
WHOLE = {
    "shell": [("top-bar", ".top", ["height"], 0), ("status-bar", ".sbar", ["height"], 0), ("sidebar", ".tree", ["width"], 0),
              ("sidebar-row", ".tree .gr", ["height"], 0), ("sidebar-label", ".tree .lab", ["height"], 0)],
    "shell-folded": [("top-bar", ".top", ["height"], 0), ("status-bar", ".sbar", ["height"], 0), ("sidebar-strip", ".strip", ["width"], 0)],
    "chat-doc": [("top-bar", ".top", [], 0), ("chat-column", ".cfcol", ["width"], 0), ("composer", ".cfcomp", [], 0)],
    "new-chat": [("top-bar", ".top", [], 0), ("chat-column", ".cfcol", ["width"], 0), ("composer", ".cfcomp", [], 0)],
    "notifications": [("notification-panel", ".npanel", ["width"], 0)],
    "menu-bar": [("menu-bar", ".mbar", ["height"], 0), ("status-menu", ".smenu", ["width"], 0)],
    "context-menu": [("context-menu", ".cmenu", ["width"], 0), ("context-menu-head", ".cmh", ["height"], 0)],
    "context-tray": [("context-tray", ".ctray", ["height"], 0)],
}
EXTRA_DEFS = {"design": [("design-header", ".dsh", ["height"], 0)]}

# Tables the page's own script defines, by the names it gives them.
DATA = [
    "T", "UM", "A1", "A2", "TOOLS", "R", "U", "G", "SN", "SUG", "AG", "SP", "LG", "LV", "LGF", "FD", "SVn",
    "AS", "AT", "CT", "CTS", "VMc", "VMS", "DMc", "DMS", "DBI", "DV", "DW", "DS", "DB", "RF", "LSc", "LSV", "DG",
    "LSD", "DGD", "PL", "PLD", "AC4", "THEMES", "TVARS", "CODE", "NEED", "FONTS", "CMX", "WEBX", "KIND", "PMETA",
    "LABM", "STATES", "TREES.map(t=>[t[0],t[1]])", "IC", "I('x',14)", "QF", "QC", "AM", "BL", "GR",
]

JS_NOTIFICATIONS = """(()=>{const p=document.querySelector('.npanel');return {head:[...p.querySelector('.h').children].map(txt),
items:[...p.querySelectorAll('.nt')].map(n=>({dot:n.querySelector('i').style.background,title:txt(n.querySelector('b')),age:txt(n.querySelector('.ago')),
text:txt(n.querySelector('.nd')),old:n.classList.contains('old'),
buttons:[...n.querySelectorAll('.ac .btn')].map(b=>({text:txt(b),kind:b.classList.contains('pri')?'primary':b.classList.contains('ghost')?'ghost':'plain'}))}))}})()"""

JS_STATUS_MENU = """(()=>{const bar=document.querySelector('.mbar');return {bar:[...bar.children].filter(c=>!c.classList.contains('smenu')).map(c=>({text:txt(c),
dot:c.querySelector('i')?c.querySelector('i').style.background:null,count:c.classList.contains('sti')})),
menu:[...bar.querySelector('.smenu').children].map(c=>c.tagName=='HR'?{type:'rule'}:c.classList.contains('h')?{type:'head',text:txt(c)}:
{type:'item',dot:c.querySelector('i')?c.querySelector('i').style.background:null,name:txt(c.querySelector('b')),note:txt(c.querySelector('.nd')),
key:txt(c.querySelector('.k')),on:c.classList.contains('on'),text:txt(c)})}})()"""

JS_AGENT_DETAIL = """(()=>{const x=document.querySelector('.agx');const k=[...x.children],pairs=[];
for(let i=0;i<k.length;i++)if(k[i].classList.contains('lb')){const v=k[i+1],m=v.querySelector('.mtr i');
pairs.push([txt(k[i]),v.classList.contains('caps')?{tags:tags(v,'.tg2')}:m?{ratio:parseFloat(m.style.width)/100,text:txt(v)}:{text:txt(v)}])}
return {pairs,buttons:[...x.querySelectorAll('.acts3 .btn')].map(b=>({text:txt(b),kind:b.classList.contains('pri')?'primary':b.classList.contains('ghost')?'ghost':'plain'}))}})()"""

JS_HUB = """(()=>{const pn=[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('Findings'));const pb=pn.querySelector('.pb');
const f2=e=>{const sv=e.querySelector('.sv'),st=e.querySelector('.stt'),mt=e.querySelector('.mt2');return {sv:txt(sv),sev:sv.className.split(' ').pop(),title:txt(e.querySelector('b')),
state:st?{label:txt(st),cls:st.className.split(' ').pop()}:null,buttons:[...e.querySelectorAll('.ta .btn')].map(txt),on:e.classList.contains('on'),
meta:mt?[...mt.children].filter(c=>txt(c)!='·').map(c=>({text:txt(c),blocks:c.classList.contains('blk'),spark:c.querySelector('.spk')?[...c.querySelectorAll('.spk i')].map(i=>parseFloat(i.style.height)):null})):[]}};
const items=[];for(const c of pb.children){if(c.classList.contains('tri')){const h=c.querySelector('.th2');items.push({type:'group',title:txt(h.querySelector('b')),note:txt(h.children[1]),extra:txt(h.children[2]),suggestions:true});c.querySelectorAll('.f2').forEach(e=>items.push({type:'finding',...f2(e)}))}
else if(c.classList.contains('grp'))items.push({type:'group',title:txt(c.querySelector('b')),note:txt(c.children[1]),extra:c.children[2]?txt(c.children[2]):''});
else if(c.classList.contains('f2'))items.push({type:'finding',...f2(c)})}
return {head:txt(pb.querySelector('.phd')),items}})()"""

JS_FINDING_DETAIL = """(()=>{const pn=[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('F12'));const b=pn.querySelector('.fdd');
const lbl=[...b.querySelectorAll('.lbl')].map(l=>({label:txt(l.childNodes[0]),note:txt(l.querySelector('span'))}));
return {id:txt(b.querySelector('.row .faint')),chips:[...b.querySelector('.row').children].map(txt),title:txt(b.querySelector('h6')),why:txt(b.querySelector(':scope > span')),
labels:lbl,evidence:tags(b,'.cchip .l'),fix:[...b.querySelectorAll('pre span')].map(s=>({sign:s.className,text:txt(s)})),
checks:[...b.querySelectorAll('.chk')].map(c=>({state:c.querySelector('i').className.split(' ').pop(),text:txt(c.children[1]),note:txt(c.children[2])})),
history:[...b.querySelectorAll('.hst > span')].map(txt),buttons:[...pn.querySelectorAll('.rwx .btn')].map(txt)}})()"""

JS_CHECKS = """(()=>{const pn=[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('Standing checks'));
return [...pn.querySelectorAll('.rul2')].map(r=>({text:txt(r.querySelector('b')),note:txt(r.querySelector('.s2')),on:r.querySelector('.tgl').classList.contains('y'),dot:r.querySelector('.sd').style.background}))})()"""

JS_SETTINGS = """(()=>{const ctl=c=>{const t=c.querySelector('.tgl'),g=c.querySelector('.seg'),s=c.querySelector('.sld'),w=c.querySelector('.sw'),d=c.querySelector('.sel2');
if(t)return {flag:t.classList.contains('y')};if(g)return {choice:txt(g.querySelector('.on')),options:tags(g,'span')};
if(s)return {ratio:parseFloat(s.querySelector('i').style.width)/100,text:txt(c.querySelector('.mono'))};if(w)return {text:txt(c.querySelector('.mono'))};
if(d)return {select:txt(d),extra:tags(c,'.chip')};return {text:txt(c)}};
return {rows:[...document.querySelectorAll('.stg')].map(s=>({title:txt(s.querySelector('b')),desc:txt(s.querySelector('.ds')),key:txt(s.querySelector('.ky span')),
scope:txt(s.querySelector('.ky span:last-child')),mod:s.classList.contains('mod'),control:ctl(s.querySelector('.ct'))})),
groups:[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('Settings')).querySelectorAll('.fx')
.length?[...[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('Settings')).querySelectorAll('.fx')].map(f=>({depth:parseInt(f.style.getPropertyValue('--d')),name:txt(f.querySelector('.nm')),count:txt(f.querySelector('.rt')),open:!!f.querySelector('.cv svg path[d^="m6 9"]')})):[]}})()"""

JS_TOKENS = """(()=>{const cs=getComputedStyle(document.querySelector('.mk .hy'));
const vars=Object.fromEntries(['--fg','--dim','--faint','--accent','--need','--nr','--ok','--bad','--kw','--ty','--lit','--nu','--cmt','--sans','--pa','--pf','--ga','--row','--ln','--gut'].map(v=>[v,cs.getPropertyValue(v).trim()]));
const pn=[...document.querySelectorAll('.mk .pn')].find(p=>txt(p.querySelector('.ptabs .pt')).startsWith('Edit theme'));
const rows=[];let grp='';for(const c of pn.querySelector('.pb').children){if(c.classList.contains('grp'))grp=txt(c);else if(c.classList.contains('tk'))rows.push({group:grp,name:txt(c.querySelector('b')),value:txt(c.querySelector('.v')),note:txt(c.querySelector('.w'))})}
return {vars,rows}})()"""

JS_CONTEXT_MENU = """(()=>{const m=document.querySelector('.cmenu');return {head:[txt(m.querySelector('.cmh b')),txt(m.querySelector('.cmh span'))],
chips:tags(m,'.cmq .chip'),items:[...m.children].filter(c=>!c.classList.contains('cmh')&&!c.classList.contains('cmq')).map(c=>c.tagName=='HR'?{type:'rule'}:
c.classList.contains('cmf')?{type:'foot',text:txt(c)}:{type:'item',text:txt(c.children[1]),key:c.children[2]?txt(c.children[2]):'',act:c.dataset.act||''})}})()"""

SCRAPES = {
    4: {"notifications": JS_NOTIFICATIONS},
    5: {"status_menu": JS_STATUS_MENU},
    6: {"agent_detail": JS_AGENT_DETAIL, "detail:trace": "DET('Trace','.spn.xr')", "detail:logs": "DET('Logs','.lgl')",
        "detail:findings-table": "DET('Findings','.fd.xr')"},
    7: {"detail:assets": "DET('Assets','.ast')", "detail:containers": "DET('Containers','.ctr')",
        "detail:vms": "DET('VMs','.vmr')", "detail:daemons": "DET('Daemons','.dmn')"},
    8: {"debugger": "DBG()"},
    9: {"detail:lsp": "DET('Language servers','.lsv')", "detail:problems": "DET('Language servers','.dgn')",
        "detail:plugins": "DET('Plugins','.plg')"},
    10: {"hub": JS_HUB, "finding_detail": JS_FINDING_DETAIL, "checks": JS_CHECKS},
    11: {"settings": JS_SETTINGS},
    12: {"tokens": JS_TOKENS},
    14: {"context_menu": JS_CONTEXT_MENU},
}
