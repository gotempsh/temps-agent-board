import { Fragment, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, Field, PageState, Status } from '@temps-sdk/ds'
import {
  Avatar, AvatarFallback, Dialog, DialogContent, DialogDescription, DialogFooter,
  DialogHeader, DialogTitle, Input, Select, SelectContent, SelectItem, SelectTrigger,
  SelectValue, Separator, Sheet, SheetContent, SheetTitle, SheetDescription, Textarea,
} from '@temps-sdk/ui'
import {
  ArrowUp, Bot, ChevronRight, Hash, ListFilter, Menu, MessageCircle, MessagesSquare,
  MessageSquare, Plus, Search, X,
} from 'lucide-react'
import { ThemeProvider } from '@/components/providers/ThemeProvider'
import './board.css'

async function call(name, args = {}) {
  const response = await fetch('/api/x/agent-board/mcp', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name, arguments: args } }),
  })
  if (!response.ok) throw Error('Board request failed (' + response.status + ')')
  const { result, error } = await response.json()
  if (error || result.isError) throw Error(error?.message || result.content[0].text)
  return JSON.parse(result.content[0].text)
}

const tones = { ready: 'idle', claimed: 'running', blocked: 'warn', done: 'ok' }
const drafts = new Map()
const timeFormat = new Intl.DateTimeFormat(undefined, {hour: 'numeric', minute: '2-digit'})
const dateFormat = new Intl.DateTimeFormat(undefined, {weekday: 'short', month: 'short', day: 'numeric', year: 'numeric'})
const replyCount = count => count === 1 ? '1 reply' : count + ' replies'

function CreateChannel({ close, created }) {
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function submit(event) {
    event.preventDefault()
    if (busy || !name.trim()) return
    setBusy(true)
    try { created((await call('create_channel', { channel: name.trim() })).name) }
    catch (failure) { setError(failure.message); setBusy(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) close() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>Create channel</DialogTitle>
        <DialogDescription>Give this conversation a place on the board.</DialogDescription></DialogHeader>
      <form onSubmit={submit} className="space-y-4">
        <Field label="Channel name" error={error}>{props =>
          <Input {...props} autoFocus required maxLength={80} placeholder="e.g. release-planning" value={name}
            onChange={event => { setName(event.target.value); setError('') }} />}</Field>
        <DialogFooter><Button type="button" variant="outline" onClick={close} disabled={busy}>Cancel</Button>
          <Button type="submit" busy={busy} disabled={!name.trim()}>Create channel</Button></DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
}

function ChannelRail({ channels, active, choose, create }) {
  return <div className="flex h-full min-h-0 flex-col bg-sidebar text-sidebar-foreground">
    <header className="flex h-16 shrink-0 items-center gap-3 border-b px-4">
      <span className="grid size-9 place-items-center rounded-lg bg-primary text-primary-foreground"><MessagesSquare className="size-5" /></span>
      <div><strong className="block text-sm">Agent Board</strong><span className="text-xs text-muted-foreground">Temps workspace</span></div>
    </header>
    <nav aria-label="Board navigation" className="flex-1 overflow-y-auto p-3">
      <Button variant={active === null ? 'secondary' : 'ghost'} className="w-full justify-start" onClick={() => choose(null)}>
        <MessagesSquare />All threads</Button>
      <div className="mt-5 mb-1 flex items-center justify-between px-2">
        <h2 className="text-xs font-semibold text-muted-foreground">Channels</h2>
        <Button variant="ghost" size="icon" className="size-8" aria-label="Create channel" title="Create channel" onClick={create}><Plus /></Button>
      </div>
      {channels.map(name => <Button key={name} variant="ghost" size="sm" aria-current={active === name ? 'page' : undefined}
        className={'mb-0.5 w-full justify-start ' + (active === name ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'text-muted-foreground')}
        onClick={() => choose(name)}><Hash /><span className="truncate">{name}</span></Button>)}
    </nav>
  </div>
}

function Composer({ draftKey, label, send, hint, autoFocus }) {
  const [value, setValue] = useState(() => drafts.get(draftKey) || '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const input = useRef(null)
  async function submit(event) {
    event.preventDefault()
    if (busy || !value.trim()) return
    setBusy(true); setError('')
    try {
      await send(value.trim())
      drafts.delete(draftKey); setValue(''); input.current?.focus()
    } catch (failure) { setError(failure.message) } finally { setBusy(false) }
  }
  return <form className="shrink-0 p-3 sm:p-4" onSubmit={submit}>
    <div className="overflow-hidden rounded-lg border bg-card shadow-sm has-[textarea:focus-visible]:outline-2 has-[textarea:focus-visible]:-outline-offset-1 has-[textarea:focus-visible]:outline-ring">
      <Textarea ref={input} autoFocus={autoFocus} aria-label={label} placeholder={label + '…'} value={value} maxLength={8192} readOnly={busy}
        className="min-h-14 resize-none rounded-none border-0 bg-transparent shadow-none focus-visible:outline-none"
        onChange={event => { setValue(event.target.value); drafts.set(draftKey, event.target.value) }}
        onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault(); event.currentTarget.form.requestSubmit()
          }
        }} />
      <div className="flex items-center justify-between gap-2 px-3 pb-2">
        <small className="text-xs text-muted-foreground">{hint}</small>
        <Button type="submit" size="sm" busy={busy} disabled={!value.trim()} aria-label={draftKey.startsWith('thread:') ? 'Send reply' : 'Send message'}>
          <ArrowUp />Send</Button>
      </div>
    </div>
    {error && <p role="alert" className="mt-1 text-sm text-destructive">{error}</p>}
  </form>
}

function Message({ item, children, selected }) {
  const date = new Date(item.created * 1000)
  const agent = item.sender?.kind === 'agent' ? item.sender : null
  const email = item.sender?.kind === 'user' ? item.sender.email : null
  return <article className={'flex gap-3 px-4 py-2.5 sm:px-5 ' + (selected ? 'bg-muted/60' : '')}>
    <Avatar className="mt-0.5 size-8 rounded-lg"><AvatarFallback className="rounded-lg text-xs font-semibold">{agent ? <Bot aria-label="Agent" className="size-4" /> : email?.slice(0, 1).toUpperCase() || `U${item.actor}`}</AvatarFallback></Avatar>
    <div className="min-w-0 flex-1">
      <div className="flex flex-wrap items-baseline gap-2"><strong className="break-all text-sm">{agent?.name ?? email ?? `User ${item.actor}`}</strong>
        <time className="text-xs text-muted-foreground" dateTime={date.toISOString()} title={date.toLocaleString()}>{timeFormat.format(date)}</time></div>
      {item.title && !item.body.startsWith(item.title) && <p className="mt-1 break-words text-sm font-semibold">{item.title}</p>}
      <p className="mt-0.5 whitespace-pre-wrap break-words text-sm leading-relaxed">{item.body}</p>
      {children}
    </div>
  </article>
}

function Divider({ children }) {
  return <div className="flex items-center gap-3 px-4 py-3 text-xs font-medium text-muted-foreground">
    <Separator className="flex-1" />{children}<Separator className="flex-1" /></div>
}

function useTail(context, last) {
  const scroll = useRef(null)
  const pinned = useRef(true)
  useLayoutEffect(() => { pinned.current = true }, [context])
  useLayoutEffect(() => {
    if (scroll.current && pinned.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [context, last])
  const onScroll = () => {
    const element = scroll.current
    pinned.current = element.scrollHeight - element.clientHeight - element.scrollTop < 48
  }
  return { ref: scroll, onScroll, pin: () => { pinned.current = true } }
}

function Thread({ task, close, refresh }) {
  const [messages, setMessages] = useState(null)
  const [error, setError] = useState('')
  const [revision, setRevision] = useState(0)
  useEffect(() => {
    let active = true
    call('read_thread', { id: task.id }).then(rows => { if (active) { setMessages(rows); setError('') } })
      .catch(failure => { if (active) setError(failure.message) })
    return () => { active = false }
  }, [task.id, task.replies, revision])
  const tail = useTail(task.id, messages?.length)
  return <section aria-label="Thread" className="flex min-h-0 min-w-0 flex-col border-l bg-background"
    onKeyDown={event => { if (event.key === 'Escape') close() }}>
    <header className="flex h-16 shrink-0 items-center gap-2 border-b px-4">
      <MessageCircle className="size-4 text-muted-foreground" /><h2 className="font-semibold">Thread</h2>
      <span className="truncate text-xs text-muted-foreground">#{task.channel}</span>
      <Button className="ml-auto" variant="ghost" size="icon" aria-label="Close thread" onClick={close}><X /></Button>
    </header>
    <div ref={tail.ref} onScroll={tail.onScroll} className="min-h-0 flex-1 overflow-y-auto pt-3">
      <Message item={task}><div className="mt-2"><Status variant="dot" tone={tones[task.status]} label={task.status} /></div></Message>
      <Divider>{replyCount(task.replies)}</Divider>
      {error && <p role="alert" className="px-4 text-sm text-destructive">{error} <Button variant="link" onClick={() => setRevision(value => value+1)}>Retry</Button></p>}
      {messages?.slice(1).map((message, index) => <Message key={index} item={message} />)}
    </div>
    <Composer autoFocus draftKey={'thread:' + task.id} label="Reply to thread" hint="Shift + Enter for a new line"
      send={async body => { await call('reply', {id:task.id, body}); tail.pin(); setRevision(value => value+1); await refresh() }} />
  </section>
}

function Board() {
  const initial = new URLSearchParams(location.search)
  const [channel, setChannel] = useState(initial.get('view') === 'all' ? null : initial.get('channel') || undefined)
  const [selected, setSelected] = useState(Number(initial.get('thread')) || null)
  const [search, setSearch] = useState(initial.get('q') || '')
  const [status, setStatus] = useState(initial.get('status') || '')
  const [filtering, setFiltering] = useState(Boolean(initial.get('q') || initial.get('status')))
  const [mobileNav, setMobileNav] = useState(false)
  const [creating, setCreating] = useState(false)
  const [data, setData] = useState(null)
  const [error, setError] = useState('')
  const serial = useRef(0)
  const opener = useRef(null)
  const refresh = useCallback(async () => {
    const version = ++serial.current
    try {
      const [channels, tasks] = await Promise.all([call('list_channels'), call('list_threads')])
      if (version === serial.current) { setData({ channels, tasks }); setError('') }
    } catch (failure) { if (version === serial.current) setError(failure.message) }
  }, [])
  useEffect(() => {
    refresh()
    const timer = setInterval(() => { if (!document.hidden) refresh() }, 5000)
    return () => clearInterval(timer)
  }, [refresh])
  const channels = data?.channels || []
  const tasks = data?.tasks || []
  const current = tasks.find(task => task.id === selected)
  const active = channel === undefined ? current?.channel || tasks[0]?.channel || channels[0] : channel
  useEffect(() => {
    if (channel === undefined && data && active) setChannel(active)
    if (!data) return
    const url = new URL(location.href)
    for (const [key, value] of Object.entries({channel:active, view:active === null ? 'all' : '', thread:selected, q:search, status})) {
      if (value) url.searchParams.set(key, value); else url.searchParams.delete(key)
    }
    history.replaceState(null, '', url)
  }, [data, active, channel, selected, search, status])
  const choose = name => {
    setChannel(name); setSelected(null); setMobileNav(false); setSearch(''); setStatus(''); setFiltering(false)
  }
  const railProps = {channels, active, choose, create: () => {setMobileNav(false); setCreating(true)}}
  const rows = tasks.filter(task => (active === null || task.channel === active) && (!status || task.status === status)
    && (task.title + ' ' + task.body).toLowerCase().includes(search.trim().toLowerCase())).reverse()
  const tail = useTail(JSON.stringify([active, search, status]), rows.at(-1)?.id)
  const filtered = Boolean(search || status)
  const close = () => { setSelected(null); requestAnimationFrame(() => opener.current?.focus()) }
  return <div className={'grid h-dvh min-h-0 grid-cols-1 overflow-hidden md:grid-cols-[13rem_minmax(0,1fr)] ' +
    (current ? 'lg:grid-cols-[13rem_minmax(0,1fr)_minmax(20rem,30%)]' : '')}>
    <aside className="hidden min-h-0 border-r md:block"><ChannelRail {...railProps} /></aside>
    <Sheet open={mobileNav} onOpenChange={setMobileNav}><SheetContent side="left" className="p-0">
      <SheetTitle className="sr-only">Channels</SheetTitle><SheetDescription className="sr-only">Choose a conversation</SheetDescription>
      <ChannelRail {...railProps} /></SheetContent></Sheet>
    <section aria-label="Conversation" className={'min-h-0 min-w-0 flex-col bg-background ' + (current ? 'hidden lg:flex' : 'flex')}>
      <header className="flex h-16 shrink-0 items-center gap-2 border-b px-3 sm:px-5">
        <Button variant="ghost" size="icon" className="md:hidden" aria-label="Open channels" onClick={() => setMobileNav(true)}><Menu /></Button>
        {active === null ? <MessagesSquare className="size-5 shrink-0 text-muted-foreground" /> : <Hash className="size-5 shrink-0 text-muted-foreground" />}
        <h1 className="min-w-0 flex-1 truncate font-semibold">{active ?? (data ? 'All threads' : 'Agent Board')}</h1>
        <Button variant={filtering?'secondary':'ghost'} size="icon" title="Search and filter" aria-label="Search and filter" aria-expanded={filtering}
          onClick={() => {setFiltering(!filtering); setSearch(''); setStatus('')}}><Search /></Button>
      </header>
      {filtering && <div className="flex flex-wrap gap-2 border-b p-3">
        <Input autoFocus className="min-w-32 flex-1" aria-label="Search messages" placeholder="Search messages…" value={search} onChange={event => setSearch(event.target.value)} />
        <Select value={status || 'all'} onValueChange={value => setStatus(value==='all'?'':value)}>
          <SelectTrigger className="w-36" aria-label="Task status"><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="all">All statuses</SelectItem>{Object.keys(tones).map(key => <SelectItem key={key} value={key}>{key}</SelectItem>)}</SelectContent>
        </Select></div>}
      {error && <p role="alert" className="px-4 text-sm text-destructive">{error} <Button variant="link" onClick={refresh}>Retry</Button></p>}
      <div ref={tail.ref} onScroll={tail.onScroll} className="min-h-0 flex-1 overflow-y-auto">
        <div className={'flex min-h-full flex-col py-3 ' + (rows.length ? 'justify-end' : 'justify-center')}>
          {rows.map((task, index) => {
            const date = dateFormat.format(task.created * 1000)
            const newDay = !index || date !== dateFormat.format(rows[index-1].created * 1000)
            return <Fragment key={task.id}>
              {newDay && <Divider>{date === dateFormat.format(Date.now()) ? 'Today' : date}</Divider>}
              <Message item={task} selected={task.id === selected}>
                <Button variant="ghost" size="sm" aria-expanded={task.id === selected} className="-ml-2 mt-1 h-7 text-xs text-muted-foreground" onClick={event => {opener.current=event.currentTarget; setSelected(task.id)}}>
                  <MessageCircle />{active === null && '#' + task.channel + ' · '}{task.replies ? replyCount(task.replies) : 'Reply in thread'}<ChevronRight />
                </Button>
              </Message></Fragment>
          })}
          {!rows.length && !error && (data ? <PageState size="compact" icon={filtered ? ListFilter : MessageSquare} variant="empty"
            title={filtered?'No matching messages':active?'Welcome to #'+active:'No conversations yet'}
            description={filtered?'Try another search or status.':active?'Send the first message below.':'Choose a channel to send a message.'}
            action={filtered && <Button variant="outline" onClick={() => {setSearch(''); setStatus('')}}>Clear filters</Button>} /> :
            <p className="p-4 text-sm text-muted-foreground">Loading conversations…</p>)}
        </div>
      </div>
      {active && <Composer autoFocus={!selected && !filtering && !creating && !mobileNav} key={active} draftKey={'channel:'+active} label={'Message #'+active} hint="Shift + Enter for a new line"
        send={async body => {
          await call('create_thread', {channel:active, title:body.split('\n')[0].slice(0,100), body})
          tail.pin(); setSearch(''); setStatus(''); await refresh()
        }} />}
    </section>
    {current && <Thread key={current.id} task={current} close={close} refresh={refresh} />}
    {creating && <CreateChannel close={() => setCreating(false)} created={name => {setCreating(false); choose(name); refresh()}} />}
  </div>
}

createRoot(document.getElementById('root')).render(
  <ThemeProvider defaultTheme="system" enableSystem attribute="class"><Board /></ThemeProvider>
)
