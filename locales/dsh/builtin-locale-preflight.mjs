// Data-only configured-presence classifier. Never imports a configured plugin,
// evaluates !!js, boots a Loader or writes a profile. Unknown is NOT absence.
import {isAbsolute,sep} from 'node:path';
const absolute=value=>typeof value==='string'&&isAbsolute(value)&&!value.split(sep).includes('..');
const legacy='@dsh-ocean/desktop-language-pack';
const builtin='@dsh-ocean/builtin-locales';
export async function classify(rows,resolveIdentity) {
 let count=0,enabled=false,disabled=false,unknown=false;
 async function visit(entries,ancestorDisabled=false,depth=0) {
  if(!Array.isArray(entries)||depth>32){unknown=true;return;}
  for(const entry of entries){
   if(++count>2048){unknown=true;return;}
   if(!entry||typeof entry!=='object'||typeof entry.name!=='string'||!entry.name||
      ![undefined,null,true,false].includes(entry.disabled)||![undefined,null,true,false].includes(entry.group)) {unknown=true;continue;}
   if(entry.group){await visit(entry.config,ancestorDisabled||entry.disabled===true,depth+1);continue;}
   if(entry.name==='cordis:include'){unknown=true;continue;}
   let identity;try{identity=await resolveIdentity(entry.name);}catch{unknown=true;continue;}
   if(!identity||typeof identity.name!=='string'){unknown=true;continue;}
   if(identity.name==='@deepseek-ai/cordis-plugin-include'){unknown=true;continue;}
   if(identity.name===legacy||identity.name===builtin){
    if(ancestorDisabled||entry.disabled===true)disabled=true;else enabled=true;
   }
  }
 }
 await visit(rows);
 return {state:unknown?'unknown':enabled?'enabled':disabled?'disabled':'absent'};
}

// SDK is passed explicitly from the selected runtime installation. The adapter
// cannot accidentally discover a developer's installed Harness or ambient home.
export async function inspectConfigured({sdk,profile,installAnchor,home,cwd,overlays=[],telemetryDisabledEnv,resolveIdentity}) {
 try {
  if(!profile||![profile.dir,profile.patchPath,installAnchor,home,cwd].every(absolute)||!Array.isArray(profile.layers)||!Array.isArray(profile.skippedBundles)||!Array.isArray(overlays)||typeof resolveIdentity!=='function')return {state:'unknown'};
  const context={name:profile.name,dir:profile.dir,patchPath:profile.patchPath,installAnchor,cwd,home,
   startedBundles:profile.layers.map(layer=>layer.packageName),overlays,telemetryDisabledEnv};
  if(profile.skippedBundles.length)return {state:'unknown'};
  let warned=false;
  const patches=sdk.readProfilePatches('ocean-locales',context,profile);
  const rows=sdk.composeEntries([patches],()=>{warned=true;});
  if(warned)return {state:'unknown'};
  const resolution=await sdk.createRuntimeResolution({installAnchor,profile,home});
  return await classify(rows,name=>resolveIdentity(name,{resolution,profile,installAnchor}));
 }catch{return {state:'unknown'};}
}
