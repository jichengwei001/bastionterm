const $ = id => document.getElementById(id);
const token = document.querySelector('meta[name="terminal-token"]').content;
const terminalOptions = {cursorBlink:true, cursorStyle:'bar', fontFamily:document.documentElement.dataset.platform==='windows'?'"JetBrains Mono", Consolas, "Microsoft YaHei UI", monospace':'"SFMono-Regular", Menlo, Consolas, monospace', fontSize:13, lineHeight:1.3, scrollback:10000, allowProposedApi:false, disableStdin:true, theme:TerminalSkins.current().terminal};
let terminal, fit;
function createTerminal(){
  terminal?.dispose(); $('terminal').replaceChildren();
  terminal=new Terminal({...terminalOptions,theme:TerminalSkins.current().terminal});fit=new FitAddon.FitAddon();
  // The SSH session answers these queries once at receipt, including when several
  // windows display it. Replaying a transcript here must never send a second reply.
  for (const id of [{final:'n'}, {prefix:'?',final:'n'}, {final:'c'}, {prefix:'>',final:'c'},
    {intermediates:'$',final:'p'}, {prefix:'?',intermediates:'$',final:'p'}]) {
    terminal.parser.registerCsiHandler(id, () => true);
  }
  terminal.parser.registerDcsHandler({intermediates:'$', final:'q'}, () => true);
  // In bundled xterm 6, OSC color reports and keyboard/paste input share onData.
  // Keep local palette changes in xterm while the SSH broker supplies the sole reply.
  const core=terminal._core.coreService, send=core.triggerDataEvent.bind(core);
  core.triggerDataEvent=(data,userInput=false)=>{
    if(!userInput && /^\x1b\](?:4;\d+|10|11|12);rgb:[0-9a-f/]+\x1b\\$/i.test(data))return;
    send(data,userInput);
  };
  terminal.loadAddon(fit);terminal.open($('terminal'));fit.fit();terminal.onData(sendInput);
}
let state = {}, actionPending = false, inputQueue = Promise.resolve(), historySignature = '', eventSource;
let profiles=[], sessions=[], activeSessionId='', tabsSignature='';
const reconnecting=new Set();
let reconnectFormPending=false;
let requestedSessionId=new URLSearchParams(location.search).get('session_id');
function message(text=''){ $('message').textContent=text; }
function confirmAction(title, detail, label){
  const dialog=$('confirm-dialog');
  if(dialog.open)return Promise.resolve(false);
  $('confirm-title').textContent=title;$('confirm-detail').textContent=detail;
  $('confirm-accept').textContent=label;dialog.returnValue='cancel';
  return new Promise(resolve=>{
    dialog.addEventListener('close',()=>resolve(dialog.returnValue==='confirm'),{once:true});
    dialog.showModal();$('confirm-cancel').focus();
  });
}
async function api(action, data={}){
  const response = await fetch('/api/'+action,{method:'POST',headers:{'Content-Type':'application/json','X-Terminal-Token':token},body:JSON.stringify(data)});
  const value=await response.json(); if(!response.ok) throw new Error(value.error || '操作失败'); return value;
}
function setControls(){
  const pending=actionPending || reconnecting.has(activeSessionId);
  const manual = state.owner==='human' && state.state==='manual';
  const connected = ['ready','busy','manual','synchronizing'].includes(state.state);
  $('takeover').hidden=state.owner==='human'; $('release').hidden=state.owner!=='human';
  $('takeover').disabled=!connected || pending; $('release').disabled=!manual || pending;
  terminal.options.disableStdin=!manual || pending;
  for(const id of ['ctrlc','escape','tab']) $(id).disabled=!manual || pending;
  $('owner').textContent=manual?'你正在操作':'Agent 操作';
  $('owner').classList.toggle('manual',manual); $('mode-dot').classList.toggle('manual',manual);
  $('hint').textContent=manual?'可直接输入 · 交还前请退出全屏程序':state.state==='disconnected'?'SSH 已断开 · 点击标签 ↻ 重连':state.state==='busy'?'Agent 正在执行 · 实时输出':state.state==='synchronizing'?'正在同步终端…':'观察模式 · 点击接管后可输入';
  $('connection').textContent=connected?'SSH 已连接':state.state==='connecting'?'SSH 登录中…':'SSH 未连接';
  $('connection-dot').classList.toggle('online',connected);
  $('route').textContent='尚未连接';$('terminal-title').textContent='SSH';
  if(state.target){
    $('route').textContent=`${state.target.host}:${state.target.port}`;
    $('route').title=`${state.target.user}@${state.target.host}:${state.target.port}`;
    $('terminal-title').textContent=`SSH · ${state.target.user}`;
  }
  syncProfileUI();
}
function drawJobs(jobs=[]){
  const signature=JSON.stringify(jobs); if(signature===historySignature)return; historySignature=signature;
  $('job-count').textContent=jobs.length; if(!jobs.length){$('jobs').replaceChildren();return;}
  $('jobs').replaceChildren(...jobs.map(job=>{
    const row=document.createElement('div'); row.className='job';
    const top=document.createElement('div');top.className='job-top';
    const stamp=document.createElement('span');stamp.textContent=job.started_at?new Date(job.started_at*1000).toLocaleTimeString('zh-CN',{hour12:false}):'—';
    const status=document.createElement('span');status.className=job.status==='running'?'running':job.exit_code===0?'success':'failed';
    status.textContent=job.status==='running'?'● 执行中':job.status==='disconnected'?'连接中断':`EXIT ${job.exit_code}`;
    const cmd=document.createElement('code');cmd.textContent=job.command;cmd.title=job.command;
    top.append(stamp,status);row.append(top,cmd);return row;
  }));
}
function drawTabs(next){
  sessions=next;
  const signature=JSON.stringify([activeSessionId,sessions,[...reconnecting]]);if(signature===tabsSignature)return;tabsSignature=signature;
  $('session-tabs').replaceChildren(...sessions.map(session=>{
    const row=document.createElement('div');row.className=`session-tab ${session.state}${session.session_id===activeSessionId?' active':''}`;
    const button=document.createElement('button');button.type='button';button.setAttribute('role','tab');
    button.setAttribute('aria-selected',String(session.session_id===activeSessionId));button.setAttribute('aria-controls','session-terminal');
    button.title=`${session.target.user}@${session.target.host}:${session.target.port}`;
    const name=document.createElement('span');name.className='tab-name';name.textContent=session.name;
    const status=document.createElement('span');status.className='tab-state';status.textContent=reconnecting.has(session.session_id)?'重连中…':session.state==='disconnected'?'已断开':session.owner==='human'?'你在操作':({ready:'就绪',busy:'执行中',connecting:'连接中',synchronizing:'同步中',disconnected:'已断开'}[session.state]||session.state);
    button.append(name,status);button.onclick=()=>selectSession(session.session_id);
    const reconnect=document.createElement('button');reconnect.type='button';reconnect.className='reconnect-session';reconnect.textContent='↻';
    reconnect.setAttribute('aria-label',`重连 ${session.name}`);
    reconnect.disabled=session.state!=='disconnected' || reconnecting.has(session.session_id);
    reconnect.title=reconnecting.has(session.session_id)?'正在重新连接…':session.state==='disconnected'?'重新连接此 SSH 主机':'仅在 SSH 断开后可重连';
    reconnect.onclick=()=>reconnectSession(session);
    const close=document.createElement('button');close.type='button';close.className='close-session';close.textContent='×';close.title='关闭此 SSH 会话';close.setAttribute('aria-label',`关闭 ${session.name}`);
    close.onclick=async()=>{
      if(actionPending || reconnecting.has(session.session_id))return;
      if(!await confirmAction(`关闭“${session.name}”？`,'将关闭此 SSH 会话并移除标签，正在运行的前台任务会中断。已保存的连接配置和密码保留。','关闭会话'))return;
      if(actionPending || reconnecting.has(session.session_id))return;
      actionPending=true;setControls();
      try{await inputQueue;await api('close-session',{session_id:session.session_id});
        drawTabs(sessions.filter(s=>s.session_id!==session.session_id));
        if(activeSessionId===session.session_id)selectSession(sessions[0]?.session_id||'');
      }catch(error){message(error.message);}finally{actionPending=false;setControls();}
    };
    close.disabled=reconnecting.has(session.session_id);
    row.append(button,reconnect,close);return row;
  }));
}
async function reconnectSession(session){
  const id=session.session_id;
  if(actionPending || reconnecting.has(id) || session.state!=='disconnected')return;
  reconnecting.add(id);drawTabs(sessions);setControls();
  try{
    const result=await api('reconnect',{session_id:id});
    if(result.needs_credentials){
      if($('login').open || reconnectFormPending){message(`“${session.name}”需要密码，请先完成当前连接配置，再点此标签重连。`);return;}
      reconnectFormPending=true;
      try{
      await loadProfiles(result.profile_id||'');
      const profile=profiles.find(p=>p.id===result.profile_id && ['host','port','user'].every(k=>String(p[k])===String(result.target[k])));
      fillProfile(profile||null);
      for(const key of ['host','port','user'])form.elements[key].value=result.target[key];
      form.elements.name.value=result.name||profile?.name||'';
      $('login-error').textContent='请补充密码以重新连接原主机。重连会创建新 shell，不会重放旧命令。';
      $('login').showModal();form.elements.password.focus();
      }finally{reconnectFormPending=false;}
    }else{
      drawTabs(sessions.map(s=>s.session_id===id?{...s,...result}:s));
      if(activeSessionId===id){state={...state,...result};setControls();resize();message('SSH 已重新连接；使用新 shell，旧命令不会重放。');}
    }
  }catch(error){message(`“${session.name}”重连失败：${error.message}`);}
  finally{reconnecting.delete(id);drawTabs(sessions);setControls();}
}
function selectSession(id){
  eventSource?.close();activeSessionId=id;state=sessions.find(s=>s.session_id===id)||{};
  historySignature='';drawJobs([]);message();createTerminal();setControls();drawTabs(sessions);
  const url=new URL(location.href);if(id)url.searchParams.set('session_id',id);else url.searchParams.delete('session_id');history.replaceState(null,'',url);
  connectEvents();resize();
}
function connectEvents(){
  const selected=activeSessionId, screen=terminal;
  const source=new EventSource('/api/events?token='+encodeURIComponent(token)+'&session_id='+encodeURIComponent(selected));eventSource=source;
  source.onopen=()=>{if(source===eventSource)screen.reset();};
  source.onmessage=event=>{
    if(source!==eventSource)return;
    const next=JSON.parse(event.data);drawTabs(next.sessions||[]);
    if(!selected && sessions.length){const id=sessions.find(s=>s.session_id===requestedSessionId)?.session_id||sessions[0].session_id;requestedSessionId=null;selectSession(id);return;}
    if(selected && !sessions.some(s=>s.session_id===selected)){selectSession(sessions[0]?.session_id||'');return;}
    if(next.error){state=next;setControls();message('会话服务暂不可用：'+next.error);return;}
    if(next.data)screen.write(next.data);
    state=next;setControls();drawJobs(next.jobs);
  };
  source.onerror=()=>{
    if(source!==eventSource)return;
    state={};setControls();$('connection').textContent='页面连接中断 · 自动重连中';
    fetch('/').then(response=>response.text()).then(html=>{
      const fresh=new DOMParser().parseFromString(html,'text/html').querySelector('meta[name="terminal-token"]')?.content;
      if(fresh && fresh!==token)location.reload();
    }).catch(()=>{});
  };
}
async function control(action){
  if(actionPending || !activeSessionId)return;const selected=activeSessionId;actionPending=true;setControls();message();
  try{await inputQueue;const result=await api(action,{session_id:selected,skin:TerminalSkins.current().id});if(activeSessionId===selected){state=result;setControls();terminal.focus();await resize();}}
  catch(error){message(error.message);}
  finally{actionPending=false;setControls();}
}
function sendInput(data){
  if(state.owner!=='human' || state.state!=='manual' || actionPending)return;
  // Preserve keyboard/paste ordering; paste and keyboard input share this queue.
  const selected=activeSessionId;const chars=Array.from(data);
  for(let i=0;i<chars.length;i+=2048){const chunk=chars.slice(i,i+2048).join('');inputQueue=inputQueue.then(()=>api('input',{session_id:selected,data:chunk})).catch(error=>message(error.message));}
}
$('takeover').onclick=()=>control('takeover');$('release').onclick=()=>control('release');
$('ctrlc').onclick=()=>{sendInput('\x03');terminal.focus();};$('escape').onclick=()=>{sendInput('\x1b');terminal.focus();};$('tab').onclick=()=>{sendInput('\t');terminal.focus();};
$('clear').onclick=()=>{terminal.clear();terminal.focus();};
$('copy').onclick=async()=>{
  let text=terminal.getSelection();
  if(!text){const buffer=terminal.buffer.active;const lines=[];for(let n=buffer.viewportY;n<Math.min(buffer.length,buffer.viewportY+terminal.rows);n++)lines.push(buffer.getLine(n).translateToString(true));text=lines.join('\n');}
  try{await navigator.clipboard.writeText(text);message('已复制终端内容。');}catch{message('请先选择终端文本，再按系统复制快捷键。');}
};
$('fullscreen').onclick=()=>document.fullscreenElement?document.exitFullscreen():document.querySelector('.terminal-shell').requestFullscreen();
$('history-toggle').onclick=()=>{
  const open=$('history-toggle').getAttribute('aria-expanded')!=='true';
  document.querySelector('main').classList.toggle('history-open',open);
  $('history-panel').hidden=!open;
  $('history-toggle').setAttribute('aria-expanded',String(open));
  const label=open?'折叠执行记录':'展开执行记录';
  $('history-toggle').setAttribute('aria-label',label);$('history-toggle').title=label;
  $('history-toggle').textContent=open?'▸':'◂';
  requestAnimationFrame(resize);
};
async function resize(){fit.fit();$('geometry').textContent=`${terminal.cols} × ${terminal.rows}`;try{if(activeSessionId)await api('resize',{session_id:activeSessionId,cols:terminal.cols,rows:terminal.rows});}catch{}}
let resizeTimer; new ResizeObserver(()=>{clearTimeout(resizeTimer);resizeTimer=setTimeout(resize,120);}).observe($('terminal'));
const form=$('login-form');
function selectedProfile(){return profiles.find(p=>p.id===$('profile-select').value);}
function matchesTarget(profile){return profile && ['host','port','user'].every(key=>String(profile[key])===form.elements[key].value.trim());}
function syncProfileUI(){
  const saved=selectedProfile();
  const reusable=saved?.has_password && matchesTarget(saved);
  form.elements.password.placeholder=reusable?'已保存，留空沿用；输入新密码可更新':'输入 SSH 密码';
  $('password-status').textContent=reusable?'密码已保存在本机配置文件。留空沿用，不会回填到页面。':'密码以明文保存在本机配置文件（权限 0600），不会回填到页面。';
  const connected=state.state && state.state!=='disconnected' && state.state!=='unavailable';
  $('profile-session-note').textContent='连接新主机会打开独立标签页；已有主机保持连接和控制权。';
  $('connect-profile').disabled=actionPending;
  $('save-profile').disabled=!form.elements.save_profile.checked || actionPending;
  $('delete-profile').disabled=!saved || actionPending;
  $('new-profile').disabled=actionPending;
  $('profile-select').disabled=actionPending;
  form.elements.remember_password.disabled=!form.elements.save_profile.checked || actionPending;
}
function fillProfile(profile){
  $('profile-select').value=profile?.id || '';
  for(const key of ['name','host','port','user'])form.elements[key].value=profile?.[key] ?? ({name:'',host:'',port:22,user:''}[key]);
  form.elements.password.value='';form.elements.save_profile.checked=true;
  form.elements.remember_password.checked=profile?profile.has_password:true;
  $('login-error').textContent='';syncProfileUI();
}
async function loadProfiles(selected){
  const result=await api('profiles');profiles=result.profiles;
  $('profile-select').replaceChildren(new Option('新建连接',''),...profiles.map(p=>new Option(p.name,p.id)));
  const chosen=selected===undefined?result.default_id:selected;
  const profile=profiles.find(p=>p.id===chosen);
  if(profile)fillProfile(profile);else if(selected==='')fillProfile(null);else syncProfileUI();
}
$('connection-settings').onclick=async()=>{
  if(actionPending || reconnectFormPending)return;actionPending=true;setControls();
  try{await loadProfiles();$('login').showModal();}catch(error){message(error.message);}
  finally{actionPending=false;setControls();}
};
$('close-settings').onclick=()=>{$('login').close();form.elements.password.value='';};
$('new-profile').onclick=()=>fillProfile(null);
$('profile-select').onchange=()=>fillProfile(selectedProfile());
form.addEventListener('input',syncProfileUI);
$('delete-profile').onclick=async()=>{
  const profile=selectedProfile();if(!profile || actionPending)return;
  if(!await confirmAction(`删除“${profile.name}”？`,'将删除此连接配置及其已保存密码，当前 SSH 会话不受影响。','删除配置'))return;
  if(actionPending)return;actionPending=true;setControls();
  try{await api('profile-delete',{id:profile.id});await loadProfiles();if(!profiles.length)fillProfile(null);$('login-error').textContent='已删除连接配置。';}
  catch(error){$('login-error').textContent=error.message;}
  finally{actionPending=false;setControls();}
};
form.onsubmit=async event=>{
  event.preventDefault();if(actionPending)return;
  const intent=event.submitter?.value || 'connect';
  const fields=Object.fromEntries(new FormData(form));
  fields.remember_password=form.elements.remember_password.checked;
  const save=form.elements.save_profile.checked;
  const saved=selectedProfile();
  actionPending=true;setControls();$('login-error').textContent='';
  try{
    let profile;
    if(save){profile=await api('profile-save',fields);await loadProfiles(profile.id);}
    if(intent==='save'){
      $('login-error').textContent=profile.has_password?'已保存连接配置和密码。':'已保存连接配置。';
    }else{
      $('connect-profile').textContent='连接中…';
      const useProfile=profile || (matchesTarget(saved)?saved:null);
      const connected=await api('connect',useProfile?{profile_id:useProfile.id,password:fields.password}:fields);
      if(!sessions.some(s=>s.session_id===connected.session_id))sessions.push(connected);
      $('login').close();selectSession(connected.session_id);
    }
  }catch(error){$('login-error').textContent=error.message;}
  finally{form.elements.password.value='';fields.password='';actionPending=false;$('connect-profile').textContent='连接远端';setControls();}
};
$('login').addEventListener('close',()=>{form.elements.password.value='';});
$('add-session').onclick=()=>$('connection-settings').click();
window.addEventListener('beforeunload',()=>eventSource?.close());
createTerminal();
$('connection-settings').disabled=true;
const fontReady=document.documentElement.dataset.platform==='windows'
  ? Promise.all([document.fonts.load('13px "JetBrains Mono"'),document.fonts.load('bold 13px "JetBrains Mono"')]).catch(()=>message('JetBrains Mono 加载失败，暂时使用系统等宽字体。'))
  : Promise.resolve();
fontReady.then(()=>{
  // Recreate before opening any stream, so xterm measures the bundled font.
  createTerminal();
  loadProfiles().catch(error=>message(error.message)).finally(()=>{$('connection-settings').disabled=false;connectEvents();resize();});
});

if(document.body.dataset.desktop==='true'){
  $('desktop-settings').hidden=false;let desktopInfo;
  async function loadDesktopInfo(){
    desktopInfo=await api('desktop-info');
    $('app-version').textContent='v'+desktopInfo.version;
    $('app-version').title='当前后台版本 '+desktopInfo.version;
    $('app-version').hidden=false;
    return desktopInfo;
  }
  loadDesktopInfo().catch(()=>{});
  function showConfig(){$('desktop-config').value=JSON.stringify(desktopInfo[$('desktop-client').value],null,2);}
  $('desktop-settings').onclick=async()=>{try{await loadDesktopInfo();$('desktop-data').textContent='数据目录：'+desktopInfo.data_dir;showConfig();$('desktop-dialog').showModal();}catch(error){message(error.message);}};
  $('desktop-client').onchange=showConfig;
  $('close-desktop').onclick=()=>$('desktop-dialog').close();
  $('copy-desktop').onclick=async()=>{try{await navigator.clipboard.writeText($('desktop-config').value);$('desktop-note').textContent='已复制。合并配置后请重启 MCP 客户端。';}catch{$('desktop-config').select();document.execCommand('copy');}};
}

function applyTerminalTheme(){
  const skin=TerminalSkins.current();
  terminal.options.theme=skin.terminal;
  if(activeSessionId)api('terminal-theme',{session_id:activeSessionId,skin:skin.id}).catch(e=>message(e.message));
}
for(const skin of TerminalSkins.all){
  const card=document.createElement('button');card.className='skin-card';card.type='button';card.dataset.skin=skin.id;
  for(const [name,value] of Object.entries(skin.ui))card.style.setProperty('--'+name,value);
  card.setAttribute('aria-label',skin.name+' · '+skin.description);
  card.innerHTML='<span class="skin-preview"><b>demo ~ ❯</b> hostname<br>ai-server <em>● ready</em></span><span class="skin-caption"></span>';
  const label=document.createElement('span');label.textContent=skin.name;
  const detail=document.createElement('small');detail.textContent=skin.description;
  card.querySelector('.skin-caption').append(label,detail);
  card.onclick=()=>{const saved=TerminalSkins.apply(skin.id);applyTerminalTheme();syncSkins();$('skin-note').textContent=saved?'已应用「'+skin.name+'」，下次打开自动沿用。':'已应用「'+skin.name+'」。当前浏览器无法保存偏好。';};
  $('skin-grid').append(card);
}
function syncSkins(){for(const card of $('skin-grid').children)card.setAttribute('aria-pressed',String(card.dataset.skin===TerminalSkins.current().id));}
syncSkins();
$('skin-settings').onclick=()=>$('skin-dialog').showModal();
$('close-skin').onclick=()=>$('skin-dialog').close();
window.addEventListener('storage',event=>{if(event.key===TerminalSkins.key){TerminalSkins.apply(event.newValue,false);applyTerminalTheme();syncSkins();}});
