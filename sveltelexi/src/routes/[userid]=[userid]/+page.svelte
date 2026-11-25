<script lang="ts">
	// let { data } = $props();
	// import {}
		import { page } from '$app/state';

	import { search } from '$lib/remote/data.remote'; 
	let person = await(search({name:page.params.userid??= "",querycount:10,strict:true}))
</script>

