<script lang="ts">
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  // import { usewsstore } from './websocketsearch';
  import '../../routes/Layout.css';
	import { onMount } from 'svelte';
  import {getlucky,search} from '$lib/remote/data.remote';
import type {
	Aliases,
	BadWordsResponse,
	PlayedWithResponse,
	Userdetails,
	Playerdeadlock
} from '$lib/morestuff/types';
  import {lucky} from './const.svelte';
	import { entries, value } from 'valibot';
  let { classNameform = '', classNameinput = '', classnamebutton = '' } = $props();

  // const { connect, disconnect, sendsearch } = usewsstore.getState();
  // onMount(() => {
  // connect()
  // sendsearch("")
  // })
  let isFocused =  $state(false);
  let matches: Array<Playerdeadlock> = $state([])
  let querycount = 0
  // console.log(matches)
  let inputRef: HTMLInputElement
  // let valuename = $derived(inputRef? inputRef.value : "woa")
  let form: HTMLFormElement;
 let steam64: string | null = null;
 let luckysteam64 = "0"
 await luckyfornexttime()
  let searchsuggestionholder: HTMLDivElement
  async function luckyfornexttime(){
      try{
    if (browser) await getlucky().refresh();
    luckysteam64 = await getlucky()
     }catch (error){
        if (browser) {
          // window.alert(`lucky is not working ${error}`);
        } else {
          console.error('Failed to load a lucky profile', error);
        }
     }
        // console.log(luckysteam64)
  }
  // $effect(() => usewsstore.subscribe((s) => (matches = s.matches)));
//  function getsteamurl(steamid: string,includefirstslash= true as boolean) {
// return `${includefirstslash &&"/"|| ""  }${encodeURIComponent(steamid)}`
//  }
  async function onSubmit(e: SubmitEvent) {
    e.preventDefault();
    inputRef?.blur();
    // disconnect();
    const username = (steam64  || new FormData(e.currentTarget as HTMLFormElement).get('user')) as string;
    steam64 = null
    console.log(username)
    console.log(matches[0])
    const match = matches.length && matches[0].personaname.toLocaleLowerCase().includes(username.toLocaleLowerCase()); 
    // I don't want that check anymore as I like the steamid being the url :)
    // nvm this check has a use
    const id = match ? matches[0].account_id.toString() : username;
    // console.log(id,"weewqewq",matches[0])
    await goto((id));
  }

  async function onsuggest(id: string, name: string) {
    // disconnect();
    steam64 = id
    inputRef.value = name
    form.requestSubmit();
    isFocused = false;
  }

  function focused() {
    // connect();
    isFocused = true;
  }
async function searchforthing(name: string){
  querycount+=1
  let stuff = await search({name,querycount,strict:false})
  if (stuff.querycount >= querycount) {
    matches = stuff.people
  }
}
function blurred(event: FocusEvent) {
    const next = event.relatedTarget as HTMLElement | null;

    if (next && searchsuggestionholder.contains(next)) {
        return;
    }

    isFocused = false;
}
let luckyid = null
 onMount( async() => {
 const lucky = getlucky();
  searchforthing("")
   let response = "0"
   try{
        await lucky.refresh();
        
         luckyid = await lucky
         
             

        }catch{
      
            // console.log("pants")
          alert(`lucky is not working ${lucky.error}`);}
 })

</script>
<div class = "flexstuff" style = "width:100%">
<div class="flexstuff flexsearchthing" onfocusout={blurred}>
  <form class={classNameform} onsubmit={onSubmit}   bind:this={form}>
  <div class = "owqdkwqdq">
    <input
      bind:this={inputRef}
      autocorrect="off"
      spellcheck="false"
      autocomplete="off"
      name="user"
      class={classNameinput}
      placeholder="Enter somones profile link, Steamid or Name"
      onfocus={(e) => { focused(); e.currentTarget.select(); searchforthing(e.currentTarget.value); }}
      oninput={(e) => { focused(); searchforthing(e.currentTarget.value); }}
    />
    <button type="button" class = "lucky" title = "I'm feeling lucky" onclick={async () => {
  //  const lucky = getlucky();
  //  let response = "0"
   
        // await lucky.refresh();
        luckyfornexttime()
        //  response = await lucky
                 await luckysteam64;

    
      
            // console.log("pants")
        
        
    }}>
    {@render lucky()} 
    </button></div>
    <button type="submit" class={classnamebutton}>Search</button>
  </form>
  <div bind:this={searchsuggestionholder} style="position: relative; width: 100%" >
    {#if isFocused} 
      <div class="searchsuggestionholder">
      <!-- {#if matches.length} -->
        {#each (matches) as value, index (index)}
          <button type="button" class="suggestion{!index ? ' importantsuggestion' : ''}" onclick={async () => await goto(value["account_id"].toString())}>
            <img
              style="height: 100%"
              src={`https://avatars.fastly.steamstatic.com/${value["avatar"] ? value["avatar"] : 'fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb'}.jpg`}
              alt=""
            />
            <div class="suggestionname">
              {value["personaname"]}
            </div>
            {#if value["match_ids"].length != 0}<span class="logcounter">{value["match_ids"].length == 1 ? '1 match' : `${value["match_ids"].length} matches`}</span>{/if}
          </button>
        {/each}
          {#if matches.length}
                  <div class="notice">Log counts are only for usernames that match the search</div>

          {:else}
             <button type="button" class="suggestion importantsuggestion" onclick={() => searchforthing(inputRef.value)}>
            <img
              style="height: 100%"
              src={`https://avatars.fastly.steamstatic.com/fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb.jpg`}
              alt=""
            />
            <div class="suggestionname">
              No results found :(
            </div>
            <!-- {#if g != 0}<span class="logcounter">{g == 1 ? '1 Log' : `${g} Logs`}</span>{/if} -->
          </button>
          {/if}
      </div>
      
    {/if}
  </div>
</div>
</div>
