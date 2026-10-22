import { readFileSync,writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { languages } from '../src/register.mjs';

export function auditCatalog(reference,translated) {
 const namespaces={},unchanged=[];let total=0,changed=0;
 for(const [namespace,dictionary] of Object.entries(reference)) {
  namespaces[namespace]=Object.keys(dictionary).length;
  for(const [key,value]of Object.entries(dictionary)) {
   total++;
   if(translated[namespace][key]===value)unchanged.push({namespace,key,value});else changed++;
  }
 }
 return {namespaces,total,changed,unchanged};
}

if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
 const data=new URL('../data/',import.meta.url),reference=JSON.parse(readFileSync(new URL('en.json',data)));
 const result=Object.fromEntries(languages.map(({id})=>[id,auditCatalog(reference,JSON.parse(readFileSync(new URL(id+'.json',data))))]));
 writeFileSync(new URL('unchanged.json',data),JSON.stringify(result,null,2)+'\n');
 for(const [id,audit]of Object.entries(result))console.log(`${id}: ${audit.total} entries, ${audit.changed} changed, ${audit.unchanged.length} explicitly listed identical entries`);
}
