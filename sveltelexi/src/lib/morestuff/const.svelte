<script module lang="ts">
  import ionLogo from "$lib/images/ion_cropped_croppedsmall.png";
  import legionLogo from "$lib/images/legion_cropped_croppedsmall.png";
  import northstarLogo from "$lib/images/northstar2_cropped_croppedsmall.png";
  import roninLogo from "$lib/images/ronin2_cropped_croppedsmall.png";
  import scorchLogo from "$lib/images/scorch2_cropped_croppedsmall.png";
  import toneLogo from "$lib/images/tone2_cropped_croppedsmall.png";
  import demomanLogo from "$lib/images/demoman.png";
  import engineerLogo from "$lib/images/engineer.png";
  import heavyweaponsLogo from "$lib/images/heavyweapons.png";
  import medicLogo from "$lib/images/medic.png";
  import pyroLogo from "$lib/images/pyro.png";
  import scoutLogo from "$lib/images/scout.png";
  import sniperLogo from "$lib/images/sniper.png";
  import soldierLogo from "$lib/images/soldier.png";
  import spyLogo from "$lib/images/spy.png";

  import abramsLogo from "$lib/images/abrams_bg_psd.png";
  import artistLogo from "$lib/images/artist_bg_psd.png";
  import astroLogo from "$lib/images/astro_bg_psd.png";
  import babaLogo from "$lib/images/baba_bg_psd.png";
  import bebopLogo from "$lib/images/bebop_bg_psd.png";
  import billyLogo from "$lib/images/billy_bg_psd.png";
  import calicoLogo from "$lib/images/calico_bg_psd.png";
  import chessmasterLogo from "$lib/images/chessmaster_bg_psd.png";
  import deadpackLogo from "$lib/images/deadpack_bg_psd.png";
  import doormanLogo from "$lib/images/doorman_bg_psd.png";
  import drifterLogo from "$lib/images/drifter_bg_psd.png";
  import dynamoLogo from "$lib/images/dynamo_bg_psd.png";
  import familiarLogo from "$lib/images/familiar_bg_psd.png";
  import fencerLogo from "$lib/images/fencer_bg_psd.png";
  import geistLogo from "$lib/images/geist_bg_psd.png";
  import genericLogo from "$lib/images/generic_bg_psd.png";
  import greyTalonLogo from "$lib/images/grey_talon_bg_psd.png";
  import hazeLogo from "$lib/images/haze_bg_psd.png";
  import infernusLogo from "$lib/images/infernus_bg_psd.png";
  import ivyLogo from "$lib/images/ivy_bg_psd.png";
  import kelvinLogo from "$lib/images/kelvin_bg_psd.png";
  import krillLogo from "$lib/images/krill_bg_psd.png";
  import lashLogo from "$lib/images/lash_bg_psd.png";
  import magicianLogo from "$lib/images/magician_bg_psd.png";
  import mcginnisLogo from "$lib/images/mcginnis_bg_psd.png";
  import minaLogo from "$lib/images/mina_bg_psd.png";
  import mirageLogo from "$lib/images/mirage_bg_psd.png";
  import necroLogo from "$lib/images/necro_bg_psd.png";
  import nurseLogo from "$lib/images/nurse_bg_psd.png";
  import paradoxLogo from "$lib/images/paradox_bg_psd.png";
  import patienceLogo from "$lib/images/patience_bg_psd.png";
  import pocketLogo from "$lib/images/pocket_bg_psd.png";
  import priestLogo from "$lib/images/priest_bg_psd.png";
  import ratkingLogo from "$lib/images/ratking_bg_psd.png";
  import sevenLogo from "$lib/images/seven_bg_psd.png";
  import shivLogo from "$lib/images/shiv_bg_psd.png";
  import unicornLogo from "$lib/images/unicorn_bg_psd.png";
  import victorLogo from "$lib/images/victor_bg_psd.png";
  import vindictaLogo from "$lib/images/vindicta_bg_psd.png";
  import viscousLogo from "$lib/images/viscous_bg_psd.png";
  import vyperLogo from "$lib/images/vyper_bg_psd.png";
  import wardenLogo from "$lib/images/warden_bg_psd.png";
  import werewolfLogo from "$lib/images/werewolf_bg_psd.png";
  import wraithLogo from "$lib/images/wraith_bg_psd.png";
  import yamatoLogo from "$lib/images/yamato_bg_psd.png";
  // import copylogo from "$lib/images/copy.svg";

  export { ClassLogo, Logo,copy ,lucky,deadLogo};

  export const deadlockLogos = [abramsLogo, artistLogo, astroLogo, babaLogo, bebopLogo, billyLogo, calicoLogo, chessmasterLogo, deadpackLogo, doormanLogo, drifterLogo, dynamoLogo, familiarLogo, fencerLogo, geistLogo, genericLogo, greyTalonLogo, hazeLogo, infernusLogo, ivyLogo, kelvinLogo, krillLogo, lashLogo, magicianLogo, mcginnisLogo, minaLogo, mirageLogo, necroLogo, nurseLogo, paradoxLogo, patienceLogo, pocketLogo, priestLogo, ratkingLogo, sevenLogo, shivLogo, unicornLogo, victorLogo, vindictaLogo, viscousLogo, vyperLogo, wardenLogo, werewolfLogo, wraithLogo, yamatoLogo] as const;

  const logos = [ionLogo, toneLogo, legionLogo, scorchLogo, roninLogo, northstarLogo] as const;
  const fallbackLogo = logos[0];
  const classLogos = {
    demoman: demomanLogo,
    engineer: engineerLogo,
    heavyweapons: heavyweaponsLogo,
    medic: medicLogo,
    pyro: pyroLogo,
    scout: scoutLogo,
    sniper: sniperLogo,
    soldier: soldierLogo,
    spy: spyLogo
  } as const;


</script>

{#snippet Logo(randomnumber =  Math.floor(Math.random() * 1000))}
<div>
      <a href="/">
        <img  src={logos[randomnumber%logos.length]} height={64} width={64} alt="" />
      </a>
    </div>
{/snippet}

{#snippet deadLogo(randomnumber =  Math.floor(Math.random() * 1000))}
<div>
      <a href="/">
        <img  src={deadlockLogos[randomnumber%deadlockLogos.length]} height={64} width={"auto"} alt="" />
      </a>
    </div>
{/snippet}



{#snippet ClassLogo(name: string)}
  {@const logo = classLogos[name.replace(/\.png$/, "").toLowerCase() as keyof typeof classLogos]}
  {#if logo}
    <img  style="height: 100%; width: 100%; object-fit:contain" src={logo} height={64} width={64} alt={name.replace(/\.png$/, "")} />
  {/if}
{/snippet}


{#snippet copy()}

     
       <svg stroke="rgba(170,170,170,1)" style="max-height:100%" xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"  stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-copy-icon lucide-copy"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>
    
  
{/snippet}


{#snippet lucky()}
          <svg fill="rgba(170,170,170,1)"xmlns="http://www.w3.org/2000/svg" height="100%" viewBox="0 -960 960 960" width="24px" ><path d="M342.5-257.5Q360-275 360-300t-17.5-42.5Q325-360 300-360t-42.5 17.5Q240-325 240-300t17.5 42.5Q275-240 300-240t42.5-17.5Zm0-360Q360-635 360-660t-17.5-42.5Q325-720 300-720t-42.5 17.5Q240-685 240-660t17.5 42.5Q275-600 300-600t42.5-17.5Zm180 180Q540-455 540-480t-17.5-42.5Q505-540 480-540t-42.5 17.5Q420-505 420-480t17.5 42.5Q455-420 480-420t42.5-17.5Zm180 180Q720-275 720-300t-17.5-42.5Q685-360 660-360t-42.5 17.5Q600-325 600-300t17.5 42.5Q635-240 660-240t42.5-17.5Zm0-360Q720-635 720-660t-17.5-42.5Q685-720 660-720t-42.5 17.5Q600-685 600-660t17.5 42.5Q635-600 660-600t42.5-17.5ZM200-120q-33 0-56.5-23.5T120-200v-560q0-33 23.5-56.5T200-840h560q33 0 56.5 23.5T840-760v560q0 33-23.5 56.5T760-120H200Zm0-80h560v-560H200v560Zm0-560v560-560Z"/></svg>
{/snippet}