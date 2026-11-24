import json
import requests
import threading
import time
import traceback
import random
import functools
from psycopg2.extras import Json
from initsql import *
from concurrent.futures import ThreadPoolExecutor, as_completed,wait, FIRST_COMPLETED
print("started")
lock = threading.Lock()
lexiesiplist = {
            "http://haste:80":{"healthy":False,"maxrequests":0},
            "http://portablefakeserver:80":{"healthy":False,"maxrequests":0},
            "http://fish.tail99fc5d.ts.net:7543":{"healthy":False,"maxrequests":0},
            "http://dublex-1:80":{"healthy":False,"maxrequests":0},
            "http://lentilserver-1:80":{"healthy":False,"maxrequests":0},
            "http://mecomputer.tailb9d154.ts.net:80":{"healthy":False,"maxrequests":0},
            "http://craptop-server.tailcb145b.ts.net:6969":{"healthy":False,"maxrequests":0},
            "http://diesel-power.tail1dda98.ts.net:7543":{"healthy":False,"maxrequests":0}
            }

# Each entry represents an outstanding /parse request from this updater.
running_tasks = {server: [] for server in lexiesiplist}


def checkifworking(firstime = False):
    # print("checking backends")
    for server in lexiesiplist:
        try:
            healthy = requests.get(f"{server}/health", timeout=5)
        except requests.RequestException:
            healthy = False
        with lock:
            # if healthy:
                # print(server,healthy.ok, healthy.json().get("max_concurrent_requests",12))
                # print({"healthy":healthy and healthy.ok,"maxrequests":(healthy or 0) and healthy.json().get("max_concurrent_requests",12)})
                # print(healthy.json().get("max_concurrent_requests",12))
            
            lexiesiplist[server] = {"healthy":(healthy or False) and healthy.ok,"maxrequests":(healthy or 0) and (healthy.ok or 0 ) and healthy.json().get("max_concurrent_requests",12)}
    if firstime:
        print(json.dumps(lexiesiplist,indent=4))
    else:
        print("backend status"," ".join(list(map(lambda x: f"{x[0].split("://",1)[1]+":":_>10} {x[1]["healthy"]}",lexiesiplist.items()))))


def parse_on_backend(matchid, payload,server):
    # attempted = set()
    # # while True:
        # with lock:
        #     candidates = [server for server, healthy in lexiesiplist.items()
        #                   if healthy["healthy"] and server not in attempted]
        #     if not candidates:
        #         time.sleep(30)
        #         break
        #     random.shuffle(candidates)  # Spread requests evenly when counts are tied.
        #     server = max(candidates, key=lambda url: lexiesiplist[url]["maxrequests"] - len(running_tasks[url]))
        #     running_tasks[server].append(matchid)
        #     active = len(running_tasks[server])

    try:
        #     attempted.add(server)
    # print("parsing match", matchid, "on", server)
        # print("asking haste for",matchid)
        response = requests.post(f"{server}/parse", json=payload, timeout=(600))
        # print(response.text)
        try:
            result = response.json()
        except:
            return (matchid,True)
        if response.status_code == 503 and result.get("error", {}).get("code") == "busy":
            print("backend busy", server, "match", matchid)
            with lock:
                lexiesiplist[server]["healthy"] = False
            return (matchid,True)
        if "results" in result:
            # try: 
            #     response = requests.get("https://api.deadlock-api.com/v1/players/steam",params={"account_ids":",".join(list(map(lambda x: x["account_id"],result["results"]["deadlock-chat-messages"]["match"]["players"])))})
                
            # except requests.ConnectionError as error:
            #     pass
            return result
        print("parse failed", matchid, server, result)
        return (matchid, True)
    except requests.ConnectionError as error:
        with lock:
            lexiesiplist[server]["healthy"] = False
        print("backend unavailable", server, "match", matchid, error)
        return (matchid, True)
    # print("no available backend for match", matchid)
    # return (matchid, True)


def dothing(matchid,server):
    try:
        # print("doing",matchid)
        now = time.time()

        r = requests.get(f"https://api.deadlock-api.com/v1/matches/{matchid}/salts",params={"disable_steam":"true"})
        if r.status_code == 429 or r.status_code == 500 or r.status_code == 404 or not r.ok:
            print("429 or 500 status code for",matchid)
            return (matchid,True)
        if "application/json" not in r.headers.get("Content-Type", ""):
            print(r.text)
            print("salt failed for",matchid)
            return (matchid, True)
        # print("got salt",matchid)

        if "demo_url" in r.json() and not r.json()["demo_url"]:
            # print("failed here2",r.text)
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(matchid,True,int(time.time()),True))
                query.commit()
            return (matchid,False)
        elif "demo_url" not in r.json():
            print("bad json for",matchid)
            return (matchid,True)

        url = r.json()["demo_url"]
        
        payload = {"url": url, "run": ["deadlock-chat-messages"]}
        result = parse_on_backend(matchid, payload,server)
        if isinstance(result, dict):
            print("Parsed demo", matchid, "in", f"{time.time()-now:.3f} seconds")
        else:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(matchid,True,int(time.time()),True))
                query.commit()
        # print("server:",server)
        # if server == "http://localhost:80":
        #     print("HEREEE")
        #     with open ("xyz.json","w") as f:
        #         json.dump(result,f,indent=4)
        return result
    except Exception:
        traceback.print_exc()
        return (matchid, True)


def dothingparttwo(matchid):
    with open("xyz.json","r") as f:
        return json.load(f)



def getnewgames():
    while True:
        with querywrapper() as query:
            query.execute("SELECT match_id FROM match WHERE parsedcorrectly = false AND (unix IS NULL OR unix < %s) ORDER BY match_id LIMIT 1000",(int(time.time())-3600*6,))
            unfinisheddownloads = query.fetchall()
            if len(unfinisheddownloads) < 10:
                query.execute("SELECT match_id FROM match ORDER BY match_id DESC LIMIT 1")
                output = query.fetchone()
                mostrecentmatchid = output and output[0] or 138067450 or 138067446
                # print(output)
                # print(mostrecentmatchid,"mostrecent")
        if len(unfinisheddownloads) < 10:
            print("doing new matches")
            r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","min_match_id":mostrecentmatchid,"max_unix_timestamp":int(time.time()-3600*5)})
            if "application/json" not in r.headers.get("Content-Type", ""):
                print(r.text)
                print("failed to parse match metadata")
                time.sleep(60)
                continue
            response = list(map(lambda x: x["match_id"],r.json()))
        else:
            print("redoing broken matches")
            # r = requests.get( 'https://api.deadlock-api.com/v1/matches/metadata',params={"include_info":"true","limit":1000,"format":"json","order_direction":"asc","match_ids":",".join(list(map(lambda x: (x[0]),unfinisheddownloads)))})
            response =list(map(lambda x: x[0],unfinisheddownloads))
        print("pulled matches")

        # yield list(map(lambda x: x["match_id"],r.json()))
        yield from  response
        

def checkifworkingstometimes():
    while True:
        time.sleep(20)
        checkifworking()

def chooseandwaitforaserver(matchid):
    with lock:
        server = max(lexiesiplist,key = lambda x: lexiesiplist[x]["maxrequests"]*int(lexiesiplist[x]["healthy"]) - len(running_tasks[x]))
        slots = lexiesiplist[server]["maxrequests"] - len(running_tasks[server])
        if slots > 0:
            running_tasks[server].append(matchid)
            print(f"doing {matchid} on {server} ({lexiesiplist[server]["maxrequests"] - len(running_tasks[server])} slots left)")

    while slots <= 0:
        with lock:
            server = max(lexiesiplist,key = lambda x: lexiesiplist[x]["maxrequests"]*int(lexiesiplist[x]["healthy"]) - len(running_tasks[x]))
            print("waiting on slot avaialble :(",lexiesiplist[server]["maxrequests"] - len(running_tasks[server]))
            slots = lexiesiplist[server]["maxrequests"] - len(running_tasks[server])
            if slots > 0:
                running_tasks[server].append(matchid)
                print(f"doing {matchid} on {server} ({lexiesiplist[server]["maxrequests"] - len(running_tasks[server])} slots left)")
                break
        time.sleep(10)
    output = (matchid,True)
    try:
        output = dothing(matchid,server)
    except:
        traceback.print_exc()
    with lock:
        running_tasks[server].remove(matchid)
    return output

def parseplayers():
    while True:
        sleeptime = 30
        with querywrapper() as query:
            query.execute("SELECT account_id FROM players WHERE last_updated IS NULL or last_updated < %s ORDER BY last_updated LIMIT 1000",(int(time.time())-86400*60,))
            output = query.fetchall()
        if not output: 
            time.sleep(120)
            continue
        if len(output) > 800:
            sleeptime = 2
        try:
            print("requesting steamids")
            response = requests.get("https://api.deadlock-api.com/v1/players/steam",params={"account_ids":",".join(list(map(lambda x: str(x[0]),output)))})
        except requests.ConnectionError as error:
            time.sleep(120)
            continue
        if not response.ok:
            print("steam request broke",response.text)
            time.sleep(120)
            continue
        try:
            now = time.time()
            print("doing",len(response.json()),"steamids")
            for person in response.json():
                # print(person)
                with querywrapper() as query:
                    query.execute("UPDATE players SET personaname = %s, profileurl = %s, avatar = %s, last_updated = %s WHERE account_id = %s",(person["personaname"],person["profileurl"],person["avatar"].rsplit("/",1)[1].split(".")[0],int(time.time()),person["account_id"]) )
                    query.commit()
            print("did steamids in",f"{time.time()-now:.3f}")
        except:
            print(response.text)
            traceback.print_exc()
        time.sleep(sleeptime)

def main():
    checkifworking(firstime=True)
    time.sleep(5)
    threading.Thread(target=parseplayers,daemon=True).start()
    threading.Thread(target=checkifworkingstometimes, daemon=True).start()
    # while True:
    #     thing = lexiesiplist.copy()
    #     # print(list(thing.values()))
    with ThreadPoolExecutor(max_workers=100) as executor:
        games = getnewgames()
        pending = set()
        while True:
            with lock:
                thing = lexiesiplist.copy()
            while len(pending) < sum(list(map(lambda x:x["maxrequests"]*int(x["healthy"]),list(thing.values())))):
                match = next(games)
                with querywrapper() as query:
                    query.execute("SELECT parsedcorrectly FROM match WHERE match_id = %s LIMIT 1",(match,))
                    output = query.fetchone()
                    if not output or not output[0]:
                        query.execute("DELETE FROM match WHERE match_id = %s",(match,))
                        query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s)",(match,False,int(time.time())))
                        query.commit()
                if output and output[0]: 
                    print("failed here - already questioned",match)
                    continue
                pending.add(executor.submit(chooseandwaitforaserver,match))
            finished,pending = wait(pending,return_when=FIRST_COMPLETED)
            for game in finished:
                try:
                    # print("e")
                    parsegame(game.result())
                except Exception:
                    traceback.print_exc()      
        # with ThreadPoolExecutor(max_workers=(sum(list(map(lambda x:x["maxrequests"]*int(x["healthy"]),list(thing.values())))))) as executor:
        #     games = [
        #         executor.submit(chooseandwaitforaserver, i)
        #         for i in getnewgames()
        #     ]

        #     for i, game in enumerate(as_completed(games)):
        #         try:
        #             parsegame(game.result(),i)
        #         except Exception:
        #             traceback.print_exc()            
                
        print("sleep time!")
        time.sleep(5)

# def my_function(i):
#     print(f"Task {i} started")
#     time.sleep(10)  # Simulate blocking for 10 seconds
#     print(f"Task {i} finished")

# with ThreadPoolExecutor(max_workers=5) as executor:
#     executor.map(my_function, range(100))

def parsegame(game,i=0):
    # print(type(game))
    if type(game) == tuple:
        print("could not parse",game)
        if game[1]:
            with querywrapper() as query:
                query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s) ON CONFLICT DO NOTHING",(game[0],False,int(time.time())))
                query.commit()
        return
    game = game["results"]["deadlock-chat-messages"]
    # print("saving game",game["match"]["match_id"])
    with querywrapper() as query:
        query.execute("SELECT table_name,column_name FROM information_schema.columns")
        rows = functools.reduce(lambda a,b: {**a,b[0]:[*a.get(b[0],[]),b[1]]}, query.fetchall(),{})
    # a is the accumulater b is the iterable
    with querywrapper() as query:
        query.execute("SELECT game_mode FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        hasalreadybeendone = query.fetchone()
        if hasalreadybeendone and hasalreadybeendone[0]:
            query.execute("INSERT INTO match (match_id,parsedcorrectly) VALUES (%s,%s) ON CONFLICT (match_id) DO UPDATE SET parsedcorrectly = %s",(game["match"]["match_id"],True,True))
            query.commit()
            print("big panic! tried to overwrite a saved row")
            return
        query.execute("DELETE FROM match WHERE match_id = %s",(game["match"]["match_id"],))
        query.execute("INSERT INTO match (match_id,parsedcorrectly,unix) VALUES (%s,%s,%s)",(game["match"]["match_id"],False,int(time.time())))
        query.commit()
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        game["match"] = {**game["match"],"parsedcorrectly":True,"account_ids": list(map(lambda x: x["account_id"],game["match"]["players"]))}
        # print(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s")
        query.execute(f"UPDATE match SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["match"],game["match"])))))} WHERE match_id = %s",(*list(map(lambda x: type(x[1]) == list and x[0] != "account_ids" and Json(x[1]) or x[1],filter(lambda x: x[0] in rows["match"],game["match"].items()))),game["match"]["match_id"]))
        query.execute("SELECT COUNT(*) FROM chat WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game["chat"]):
            for chat_number, data in enumerate(game["chat"]):
                data = {**game["match"],**data,"chat_number":chat_number}
                # print(data)
                # print(rows[table])
                # print(list(filter(lambda x: x in rows[table],data)))
                # print(f"INSERT INTO {table} ({",".join(list(filter(lambda x: x in rows[table],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows[table],data))))})")
                # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows[table],data.items()))))
                query.execute(f"INSERT INTO {"chat"} ({",".join(list(filter(lambda x: x in rows["chat"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["chat"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["chat"],data.items()))))
        query.execute("SELECT COUNT(*) FROM items WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game.get("items",[])):
            for item_number, data in enumerate(game["items"]):
                data = {**game["match"],**data,"item_number":item_number}#,"imbuedability":Json(data["imbuedability"])}
                query.execute(f"INSERT INTO {"items"} ({",".join(list(filter(lambda x: x in rows["items"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["items"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["items"],data.items()))))
        query.execute("SELECT COUNT(*) FROM events WHERE match_id = %s",(game["match"]["match_id"],))
        output = query.fetchone()
        if not output or output[0] != len(game["events"]):
            for event_number,data in enumerate(game["events"]):
                
                # print(list(map(lambda x: x.removeprefix("attacker_") ,rows["events"])),"wqdwqdwq")
                # print("eee",data["assists"])
                for i,thing in enumerate(data["assists"] or []) :
                    data["assists"][i] = dict(filter(lambda x: x[0] in list(map(lambda x: x.removeprefix("killer_") ,rows["events"])),thing.items()))
                data = {**game["match"],**data,"event_number":event_number,"assists":Json(data["assists"])}
                # print(json.dumps(data,indent=4))
                # break
                # print(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})")
                # print(list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
                query.execute(f"INSERT INTO {"events"} ({",".join(list(filter(lambda x: x in rows["events"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["events"],data))))})",list(map(lambda x: x[1],filter(lambda x: x[0] in rows["events"],data.items()))))
        for data in game["match"]["players"]:
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],data)))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s" ,list(filter(lambda x: x in rows["players"],data)))))}")
            # print(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],data))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s")
            # print((*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
            query.execute(f"INSERT INTO {"players"} ({",".join(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]})))}) VALUES ({",".join(["%s"]*len(list(filter(lambda x: x in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}))))}) ON CONFLICT (account_id) DO UPDATE SET {",".join(list(map(lambda x: f"{x} = %s",list(filter(lambda x: x in rows["players"],data)))))}, match_ids = players.match_ids || %s",(*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items()))),*list(map(lambda x: x[1],filter(lambda x: x[0] in rows["players"],{**data,"match_ids":[game["match"]["match_id"]]}.items())))))
        query.commit()
    print("saved game",game["match"]["match_id"],(int(time.time()-game["match"]["unix"])//3600),"h", int(((time.time()-game["match"]["unix"]) % 3600) // 60),"m", int((time.time()-game["match"]["unix"]) % 60),"s behind")
        
        

    


if __name__ == "__main__":
    main()
