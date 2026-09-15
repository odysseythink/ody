// ody canvas bridge 注入插件（切片 3，协议与 odyBox
// src/renderer/components/artifacts/sandbox.ts runtimeBridgeScript() 逐字一致，
// 两端必须同步修改）。用 Vite transformIndexHtml 把 bridge 脚本注入被服务
// 的 HTML——不包裹 HTML（dev server 的 CSP/HMR 不容包裹），用户文件零改动。
const BRIDGE_JS = `(function(){
    const SOURCE='odybox-canvas-preview';
    const send=(type,payload={})=>parent.postMessage({source:SOURCE,type,...payload},'*');
    const runtimeErrors=[];
    let interactionMode='off';
    let hoverOverlay;
    const recordError=message=>{runtimeErrors.push(String(message).slice(0,500));send('runtime-error',{message:String(message)});};
    window.addEventListener('error',event=>recordError(event.message||'Unknown runtime error'));
    window.addEventListener('unhandledrejection',event=>recordError(event.reason?.message||event.reason||'Unhandled promise rejection'));
    window.addEventListener('load',()=>send('ready'));

    const inspectable=target=>target instanceof Element?target.closest('[data-ody-id]'):null;
    const stylesFor=element=>{const style=getComputedStyle(element);return {
      display:style.display,position:style.position,color:style.color,backgroundColor:style.backgroundColor,
      fontFamily:style.fontFamily,fontSize:style.fontSize,fontWeight:style.fontWeight,lineHeight:style.lineHeight,
      margin:style.margin,padding:style.padding,border:style.border,borderRadius:style.borderRadius,
      width:style.width,height:style.height,gap:style.gap,alignItems:style.alignItems,justifyContent:style.justifyContent
    };};
    const describe=element=>{const rect=element.getBoundingClientRect();return {
      nodeId:element.getAttribute('data-ody-id'),tagName:element.tagName.toLowerCase(),
      role:element.getAttribute('role')||undefined,
      accessibleName:(element.getAttribute('aria-label')||element.getAttribute('alt')||element.textContent||'').replace(/\s+/g,' ').trim().slice(0,120)||undefined,
      textPreview:(element.textContent||'').replace(/\s+/g,' ').trim().slice(0,160)||undefined,
      bounds:{x:rect.x,y:rect.y,width:rect.width,height:rect.height},computedStyles:stylesFor(element)
    };};
    const showHover=element=>{
      if(!hoverOverlay){hoverOverlay=document.createElement('div');hoverOverlay.dataset.odyboxOverlay='hover';Object.assign(hoverOverlay.style,{position:'fixed',pointerEvents:'none',zIndex:'2147483646',border:'2px solid #4c6ef5',background:'rgba(76,110,245,.10)'});document.documentElement.appendChild(hoverOverlay);}
      if(!element){hoverOverlay.style.display='none';return;}
      const rect=element.getBoundingClientRect();Object.assign(hoverOverlay.style,{display:'block',left:rect.left+'px',top:rect.top+'px',width:rect.width+'px',height:rect.height+'px'});
    };
    document.addEventListener('mousemove',event=>{if(interactionMode!=='inspect')return;const element=inspectable(event.target);showHover(element);if(element)send('element-hover',{element:describe(element)});},true);
    document.addEventListener('click',event=>{
      if(interactionMode==='off')return;
      const element=inspectable(event.target);
      if(interactionMode==='inspect'&&element){event.preventDefault();event.stopPropagation();showHover(element);send('element-selected',{element:describe(element)});}
      if(interactionMode==='pin'){
        event.preventDefault();event.stopPropagation();
        const width=Math.max(document.documentElement.scrollWidth,1),height=Math.max(document.documentElement.scrollHeight,1);
        send('free-pin',{xRatio:Math.max(0,Math.min(1,event.pageX/width)),yRatio:Math.max(0,Math.min(1,event.pageY/height)),element:element?describe(element):undefined});
      }
    },true);

    const audit=viewport=>{
      const issues=[];
      const add=(code,message,element,severity='warning')=>issues.push({code,message,severity,nodeId:element?.getAttribute?.('data-ody-id')||undefined});
      document.querySelectorAll('img:not([alt])').forEach(element=>add('image_missing_alt','Image is missing alt text',element,'error'));
      document.querySelectorAll('button,a[href],input,select,textarea').forEach(element=>{
        const name=(element.getAttribute('aria-label')||element.getAttribute('title')||element.textContent||element.getAttribute('placeholder')||'').trim();
        if(!name)add('control_missing_name','Interactive control has no accessible name',element,'error');
      });
      document.querySelectorAll('input,select,textarea').forEach(element=>{
        const id=element.getAttribute('id');
        if(!element.getAttribute('aria-label')&&!(id&&document.querySelector('label[for="'+CSS.escape(id)+'"]')))add('field_missing_label','Form field has no label',element,'error');
      });
      const ids=new Set();document.querySelectorAll('[id]').forEach(element=>{const id=element.id;if(ids.has(id))add('duplicate_id','Duplicate HTML id: '+id,element,'error');ids.add(id);});
      if(document.documentElement.scrollWidth>document.documentElement.clientWidth+2)add('horizontal_overflow','Page overflows horizontally at this viewport',document.body,'warning');
      return {viewport,width:document.documentElement.clientWidth,height:document.documentElement.clientHeight,issues,runtimeErrors:[...runtimeErrors],auditedAt:Date.now()};
    };

    const renderAnnotations=annotations=>{
      document.querySelectorAll('[data-odybox-overlay="annotation"]').forEach(node=>node.remove());
      for(const annotation of annotations||[]){
        const marker=document.createElement('button');marker.type='button';marker.dataset.odyboxOverlay='annotation';marker.textContent=String(annotation.index);
        const target=annotation.nodeId?Array.from(document.querySelectorAll('[data-ody-id]')).find(node=>node.getAttribute('data-ody-id')===annotation.nodeId):null;
        const rect=target?.getBoundingClientRect();
        const left=rect?(rect.right+window.scrollX):((annotation.xRatio||0)*Math.max(document.documentElement.scrollWidth,1));
        const top=rect?(rect.top+window.scrollY):((annotation.yRatio||0)*Math.max(document.documentElement.scrollHeight,1));
        marker.title=annotation.body||'Comment';Object.assign(marker.style,{position:'absolute',left:left+'px',top:top+'px',transform:'translate(-50%,-50%)',zIndex:'2147483645',width:'24px',height:'24px',borderRadius:'999px',border:'2px solid white',background:'#fa5252',color:'white',font:'600 12px system-ui',boxShadow:'0 2px 8px rgba(0,0,0,.35)'});
        document.body.appendChild(marker);
      }
    };

    window.addEventListener('message',async event=>{
      if(event.source!==parent||event.data?.source!=='odybox-canvas-host')return;
      if(event.data.type==='interaction-mode'){interactionMode=event.data.mode||'off';if(interactionMode!=='inspect')showHover(null);return;}
      if(event.data.type==='annotations'){renderAnnotations(event.data.annotations);return;}
      const requestId=event.data.requestId;
      if(event.data.type==='audit'){send('audit-result',{requestId,result:audit(event.data.viewport)});return;}
      if(event.data.type!=='snapshot')return;
      try {
        const width=Math.max(document.documentElement.scrollWidth,document.documentElement.clientWidth,320);
        const height=Math.max(document.documentElement.scrollHeight,document.documentElement.clientHeight,240);
        const snapshotRoot=document.documentElement.cloneNode(true);
        snapshotRoot.querySelectorAll('[data-odybox-overlay],script[data-odybox-canvas-bridge]').forEach(node=>node.remove());
        const serialized=new XMLSerializer().serializeToString(snapshotRoot);
        const svg='<svg xmlns="http://www.w3.org/2000/svg" width="'+width+'" height="'+height+'"><foreignObject width="100%" height="100%">'+serialized+'</foreignObject></svg>';
        const url=URL.createObjectURL(new Blob([svg],{type:'image/svg+xml;charset=utf-8'}));
        const image=new Image();
        image.onload=()=>{
          try {
            const canvas=document.createElement('canvas');
            canvas.width=width;canvas.height=height;
            canvas.getContext('2d').drawImage(image,0,0);
            send('snapshot-result',{requestId,dataUrl:canvas.toDataURL('image/png')});
          } catch(error) { send('snapshot-error',{requestId,message:String(error?.message||error)}); }
          finally { URL.revokeObjectURL(url); }
        };
        image.onerror=()=>{URL.revokeObjectURL(url);send('snapshot-error',{requestId,message:'Preview could not be rendered as an image'});};
        image.src=url;
      } catch(error) { send('snapshot-error',{requestId,message:String(error?.message||error)}); }
    });
  })();`

export default function odyCanvasBridgePlugin() {
  return {
    name: 'ody-canvas-bridge',
    transformIndexHtml: {
      order: 'post',
      handler(html) {
        if (html.includes('data-odybox-canvas-bridge')) return html
        return [
          {
            tag: 'script',
            attrs: { 'data-odybox-canvas-bridge': '' },
            children: BRIDGE_JS,
            injectTo: 'body',
          },
        ]
      },
    },
  }
}
