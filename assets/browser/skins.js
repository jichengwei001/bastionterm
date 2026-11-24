/* Apply before first paint; palettes contain no remote assets or user data. */
window.TerminalSkins=(()=>{
  document.documentElement.dataset.platform=/Win/i.test(navigator.platform)?'windows':'other';
  const key='bastion-bridge.skin';
  const definitions=[
    ['tundra','苔原绿','沉静 · 深色','dark', ['#101311','#181c19','#2b322d','#e1e7e0','#91a095','#b5e68b','#e7b96f','#0c100d','#29332a','#586a57','#a9b6ab','#1a2816','#caefa9','#101610','#e3a38f'], ['#df9685','#b5d99a','#d4be84','#8db4c9','#c5a4bf','#92c6bb']],
    ['ocean','深海蓝','冷冽 · 深色','dark', ['#101923','#172330','#2c3e50','#dfebf5','#93a9bc','#7dcfff','#f0c674','#0c141d','#22364a','#4d6981','#afc3d5','#092438','#a6dfff','#101c28','#f29a95'], ['#f29a95','#9ed6b3','#f0c674','#7dcfff','#b5a9e8','#75d5d0']],
    ['graphite','石墨黑','纯粹 · 深色','dark', ['#171719','#222225','#38383d','#ededf0','#a2a2ad','#d1d1df','#e4b980','#111113','#303034','#60606b','#c0c0ca','#222228','#f0f0f7','#19191c','#eba2a7'], ['#eba2a7','#b1cba4','#e4c28f','#a4bce2','#c8aee0','#a0c9ca']],
    ['paper','暖纸白','柔和 · 浅色','light', ['#f5f1e8','#ebe6da','#d1c8b7','#3b352a','#746a59','#47613a','#8d570d','#fbf8f0','#e4ddce','#a89b83','#625744','#ffffff','#38502d','#fffdf8','#a13830'], ['#a13830','#426436','#8d640e','#355e85','#805282','#276a63']],
    ['daylight','晴空白','清爽 · 浅色','light', ['#f1f5f9','#e6edf4','#c7d4e1','#20334a','#5c7086','#23629b','#925315','#ffffff','#d9e5f0','#91a8bf','#425f7a','#ffffff','#194e7d','#f8fbfe','#b03c48'], ['#b03c48','#2c714a','#8b6416','#23629b','#8153a0','#21777d']]
  ];
  const names=['bg','panel','border','text','muted','green','amber','terminal-bg','hover','border-strong','secondary','on-accent','accent-hover','input-bg','danger'];
  const all=definitions.map(([id,name,description,scheme,values,ansi])=>{
    const ui=Object.fromEntries(names.map((name,i)=>[name,values[i]]));
    ui.backdrop=scheme==='dark'?'#03070bcc':'#24344766';
    const terminal={background:ui['terminal-bg'],foreground:ui.text,cursor:ui.green,cursorAccent:ui['terminal-bg'],selectionBackground:ui.green+'40',black:scheme==='dark'?ui.panel:'#263342',white:scheme==='dark'?ui.text:'#c4ccd2',brightBlack:ui.muted,brightWhite:scheme==='dark'?'#ffffff':'#e5e9ed'};
    ['red','green','yellow','blue','magenta','cyan'].forEach((color,i)=>{terminal[color]=ansi[i];terminal['bright'+color[0].toUpperCase()+color.slice(1)]=ansi[i];});
    return {id,name,description,scheme,ui,terminal};
  });
  let active=all[0];
  function apply(id,persist=true){
    active=all.find(skin=>skin.id===id)||all[0];
    const root=document.documentElement;root.dataset.skin=active.id;root.style.colorScheme=active.scheme;
    for(const [name,value] of Object.entries(active.ui))root.style.setProperty('--'+name,value);
    if(persist){try{localStorage.setItem(key,active.id);}catch{return false;}}
    return true;
  }
  let saved;try{saved=localStorage.getItem(key);}catch{}
  apply(saved,false);
  return {key,all,current:()=>active,apply};
})();
